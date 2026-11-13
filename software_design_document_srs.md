# Specyfikacja Techniczna i Architektura Systemu (SDD & SRS)

## Silnik Routingu Pogodowego Jachtów Żaglowych (Routing Engine Core)

## 1. Przegląd i Architektura Wysokopoziomowa (High-Level Architecture)

### 1.1. Diagram Architektury

Architektura systemu opiera się na wydzieleniu warstwy asymilacji danych (I/O heavy) od warstwy obliczeniowej (CPU/Memory heavy).

```
graph TD
    subgraph Client Layer
        Client[Client / Frontend / NMEA Integration]
    end

    subgraph API Gateway & Orchestration
        API[gRPC / REST Gateway]
        JobQueue[Task Queue / Redis]
    end

    subgraph Core Routing Engine
        Router1[Routing Worker Node 1]
        Router2[Routing Worker Node N]
    end

    subgraph Data Ingestion & Storage
        WeatherIngest[Weather Ingestion Service]
        VPPService[Vessel Performance Service]
        
        S3[(Object Storage \n GRIB2 / NetCDF / Zarr)]
        PG[(PostGIS \n S-57 Coastlines / No-Go Zones)]
        Cache[(Redis \n VPP Cache / Job State)]
    end
    
    subgraph In-Memory Data Structures
        RAM_Meteo[mmap Zarr Grids]
        RAM_Spatial[R-Tree / S2 Spatial Index]
    end

    Client -->|gRPC/Protobuf/JSON| API
    API --> JobQueue
    JobQueue --> Router1
    JobQueue --> Router2
    
    WeatherIngest -->|Decode & Chunk| S3
    Router1 <==>|Zero-copy Read| RAM_Meteo
    S3 -.->|Load/Sync| RAM_Meteo
    
    PG -.->|Pre-load at startup| RAM_Spatial
    Router1 <==> RAM_Spatial
    
    VPPService --> Cache
    Router1 --> Cache

```

### 1.2. Rekomendowany stos technologiczny (Tech Stack)

* **Core Obliczeniowy:** `Rust`. Uzasadnienie: Bezpieczeństwo pamięci przy wielowątkowości, brak Garbage Collectora (przewidywalne opóźnienia P99), łatwy dostęp do instrukcji wektoryzacji SIMD (AVX2/AVX-512) niezbędnych przy bilinearnych operacjach na wielomilionowych siatkach. Alternatywą jest `C++20`.

* **Ingestia Pogody:** `Python` + `xarray` + `eccodes`. Uzasadnienie: Najlepszy ekosystem do parsowania GRIB2 (z dekompresją AEC/JPEG2000) i NetCDF. Skrypty pythonowe parsują, transponują do formatu Zarr i wrzucają na Object Storage.

* **Baza Danych Przestrzennych:** `PostgreSQL` + `PostGIS`. Uzasadnienie: Przechowywanie źródłowych linii brzegowych (S-57). Podczas uruchamiania (startup) workerów Rust, dane są pobierane i ładowane do in-memory R-Tree.

* **Siatki Przestrzenne (In-Memory):** `H3` (Uber) lub indeksowanie za pomocą `R-Tree` (np. biblioteka `rstar` w Rust) dla błyskawicznego broad-phase collision detection.

* **Cache / Kolejkowanie:** `Redis`. Przechowuje sparowane biegunowe (VPP) i obsługuje kolejki zadań (asynchroniczny routing).

### 1.3. Model Przetwarzania

* **Zapytania synchroniczne REST:** Trasy krótsze niż 200 Mm są obliczane synchronicznie i zwracają pełną trasę. Pozostałe żądania REST zwracają HTTP 202 i `job_id`, a wynik jest odczytywany przez endpoint statusu.

* **gRPC Server Streaming:** `CalculateRoute` emituje częściowe izochrony w miarę propagacji, a następnie końcowy wynik. Kontrakt MVP nie wysyła webhooków.

## 2. Wymagania Funkcjonalne (Functional Requirements)

### Moduł A: Zarządzanie danymi środowiskowymi

| ID | Nazwa | Priorytet | Opis logiczny i Kryteria Akceptacji | 
| ----- | ----- | ----- | ----- | 
| **FR-001** | Dekodowanie i transformacja GRIB2 | MVP | **Logika:** Usługa asymilacji pobiera GRIB2 (GFS, ECMWF, RTOFS), dekoduje skompresowane dane (JPEG2000), konwertuje na format Zarr z chunkingiem w osi czasu. **AC:** Składowe wiatru $U, V$ (10m), MSLP, składowe fali $H_s, T_p, Dir$, prądy $U, V$ są poprawne i znormalizowane. | 
| **FR-002** | Interpolacja 4D z korektą antypołudnika | MVP | **Logika:** Silnik interpoluje parametry używając interpolacji trójliniowej (bilinear w $X,Y$, linear w $T$). **AC:** Brak błędu "wrap-around" dla kierunków wiatru (obliczenia na wektorach $U/V$); dystanse wokół południka 180° rzutowane precyzyjnie (delta modulo $2\pi$). | 
| **FR-003** | Grid Blending i Ensemble Routing | Pro | **Logika:** Płynne przenikanie gęstych modeli lokalnych (np. AROME 0.01°) w globalne (GFS 0.25°) przy użyciu sigmoidalnej wagi dystansu od granicy. Zespół Izochron dla wiązki EPS z progiem CVaR dla odrzucania ryzyk. **AC:** Pochodne kierunkowe na granicach modeli są ciągłe; rzadkie, niebezpieczne odchylenia meteo odcinają krawędzie trasy. | 

### Moduł B: Modelowanie Osiągów Jachtu (VPP)

| ID | Nazwa | Priorytet | Opis logiczny i Kryteria Akceptacji | 
| ----- | ----- | ----- | ----- | 
| **FR-004** | Ewaluacja STW (Speed Through Water) | MVP | **Logika:** Interpolacja dwuliniowa na zdekodowanej macierzy VPP (TWS/TWA). Poniżej dolnego progu wiatru VPP, funkcja skalowania kwadratowego symuluje utratę przyczepności (stall). **AC:** Prędkość poprawnie wyliczona dla ułamkowych kątów i prędkości. | 
| **FR-005** | Motor-sailing i Kary (Penalties) | MVP | **Logika:** Nakładanie deterministycznej straty pędu na krawędzie wymuszające zmianę halsu (Tack/Gybe). Jeśli $STW < STW_{min}$, flaga `motorState = true`, $SOG$ = prędkość przelotowa, $Fuel = Fuel + (Rate \cdot \Delta t)$. **AC:** Silnik odrzuca krawędzie po wyczerpaniu baku. | 
| **FR-006** | Dodatkowy opór falowy ($R_{added}$) | Pro | **Logika:** Wdrożenie ISO 15016 / metody Kwona. Redukcja uciągu VPP na bazie wysokości fali i okresu szczytowego ($T_p$) względem częstotliwości uderzeń kadłuba. **AC:** Znaczny spadek estymowanej prędkości na kursach ostrych przy dużej fali. | 

### Moduł C: Silnik Izochron i Optymalizacja

| ID | Nazwa | Priorytet | Opis logiczny i Kryteria Akceptacji | 
| ----- | ----- | ----- | ----- | 
| **FR-007** | Propagacja Zmodyfikowanej Izochrony | MVP | **Logika:** Krokowa ekspansja węzłów co $\Delta t$. Przestrzeń celowa podzielona na "bearing buckets" (np. co 1°). Zostaje tylko jeden, optymalny węzeł na bucket. **AC:** Brak samoprzecięć (loops), stała złożoność czasowa dla każdego kroku czasowego. | 
| **FR-008** | No-Sail Zone Tapering | MVP | **Logika:** Wektor wiatru w sektorze martwym (np. $\pm 35^\circ$ od linii wiatru) wymnaża prędkość przez rampę wygasającą do zera. **AC:** Silnik nie dopuszcza bezpośrednich rzutów węzłów w środek martwego kąta (wymusza wężykowanie - beating). | 
| **FR-009** | Korytarz Heurystyczny (A\* Pre-process) | Pro | **Logika:** Zgrubne przejście A\* (z funkcją kosztu $H(x) = dist / V_{max}$) dla wygenerowania poligonu ograniczającego (korytarza) dla izochron. **AC:** Bezbłędne przejścia przez wąskie cieśniny, ignorowanie ślepych zatok przez Izochrony. | 

### Moduł D: Analiza Zaawansowana

| ID | Nazwa | Priorytet | Opis logiczny i Kryteria Akceptacji | 
| ----- | ----- | ----- | ----- | 
| **FR-010** | Dynamic Re-routing | MVP | **Logika:** Trigger wywoływany asynchronicznie, gdy różnica między estymowanym XTE (Cross-Track Error) a danymi z szyny NMEA 2000 (SOG, COG) przekroczy zadany limit. **AC:** Automatyczne przeliczenie z nowego punktu $P_0$ bez zmiany celu. | 

## 3. Szczegółowy Projekt Algorytmów i Logiki Obliczeniowej

### 3.1. Przepływ głównego algorytmu routingu (Modified Isochrones)

```
# Pseudokod struktury głównej pętli (w implementacji - Rust z wektoryzacją)
def calculate_isochrones(start_node, target, grid, vpp, config):
    active_front = [start_node]
    isochrone_history = []
    
    while not reached(active_front, target) and not timeout():
        next_front_candidates = []
        
        for node in active_front:
            env_data = grid.get_4d_point(node.lat, node.lon, node.time)
            
            # Wektoryzacja: ewaluacja n kandydujących kątów kursowych (HDG) naraz
            for hdg in config.hdg_spread:
                # 1. TWA i TWS z wektorów
                twa, tws = calculate_true_wind(env_data.wind_u, env_data.wind_v, hdg, node.sog)
                
                # 2. Ewaluacja No-Sail Zone
                if is_in_no_sail_zone(twa, config.min_twa):
                    continue
                    
                # 3. Model Fizyczny
                stw = vpp.get_speed(twa, tws)
                stw_reduced = apply_wave_resistance(stw, env_data.wave_h, env_data.wave_dir, hdg)
                sog, cog = vector_compose(stw_reduced, hdg, env_data.curr_u, env_data.curr_v, leeway_model(stw, twa))
                
                # 4. Motor-Sailing Check
                fuel_used = 0
                if sog < config.min_motoring_sog and node.fuel_left > 0:
                    sog = config.motoring_speed
                    cog = hdg # uproszczenie
                    fuel_used = config.fuel_rate * config.delta_t
                
                # 5. Geometria i Kolizje (Broad & Narrow Phase)
                next_lat, next_lon = haversine_project(node.lat, node.lon, cog, sog * config.delta_t)
                if spatial_index.intersects_land_or_nogo(node, (next_lat, next_lon)):
                    continue # Pruning krawędzi
                
                # 6. Płycizny i UKC
                ukc = evaluate_ukc(next_lat, next_lon, sog, node.heel_angle)
                if ukc < config.min_ukc:
                    continue
                    
                new_node = Node(next_lat, next_lon, time + delta_t, sog, cog, fuel_used, parent=node)
                next_front_candidates.append(new_node)
        
        # 7. Redukcja przez Bearing Buckets (Dystans do celu jako heurystyka)
        active_front = filter_by_bearing_buckets(next_front_candidates, target, resolution=1.0)
        isochrone_history.append(active_front)
        
    return backtrack_optimal_route(active_front, target)

```

### 3.2. Matematyka i fizyka ruchu

Silnik w warstwie C++/Rust rozwiązuje poniższe równania z zastosowaniem arytmetyki zmiennoprzecinkowej podwójnej precyzji (`f64`).

**1. Składanie wektorów prędkości (SOG / COG):**
Ostateczny wektor ruchu względem dna $\vec{V}_{SOG}$ jest sumą wektora prędkości po wodzie (STW), wektora dryfu (Leeway) oraz wektora prądu morskiego:

$$
\vec{V}_{SOG} = \vec{V}_{STW}(TWA, TWS) + \vec{V}_{Leeway} + \vec{V}_{Current}
$$


Gdzie moduł dryfu aproksymowany jest modelem klasycznym: $Leeway = K \cdot \frac{Heel}{(STW)^2}$ (współczynnik $K$ empiryczny kadłuba).

**2. Osiadanie (Squat) dla wąskich kanałów (Wzór Barrassa):**
Aby zapobiec wejściu jachtu na mieliznę pod wpływem zjawisk hydrodynamicznych, dynamiczne zanurzenie przyrasta o:

$$
Squat = \frac{C_b \cdot (\vec{V}_{STW})^2}{100}
$$


gdzie $C_b$ to Block Coefficient jachtu, a prędkość wyrażona jest w węzłach.

Opór falowy i korekta batymetrii o pływy są pominięte w MVP; ich przyszłe
wymagania zapisano w `BYPASS_FUTURES.md`. Analityk podał bezpieczną głębokość
`5 m` bez pływów i zanurzenie statyczne jednostki `2 m`, lecz nie rozstrzygnął,
czy 5 m oznacza głębokość batymetryczną czy pozostały UKC. Dynamiczne UKC/squat
pozostaje zablokowane do czasu doprecyzowania tej reguły i wskazania źródła
współczynnika blokowego `C_b` oraz szerokości kadłuba dla `vessel_id`
(`ANALYST_QUESTIONS.md`).

**3. Odległość Ortotromowa z korektą Antypołudnika:**
Rzutowanie pozycji używa modyfikacji długości geograficznej:

$$
\Delta\lambda = ((\lambda_2 - \lambda_1 + \pi) \bmod 2\pi) - \pi
$$

### 3.3. Detekcja kolizji i przestrzeń (Spatial Engine)

Dla zapewnienia wydajności $< 1 \text{ ms}$ na tysiąc krawędzi, system operuje w oparciu o pamięciowe struktury R-Tree z dwufazowym sprawdzaniem:

1. **Broad-Phase (Bounding Boxes):** Silnik rzutuje linię pomiędzy $P_{start}$ a $P_{end}$ i otacza ją minimalnym prostokątem (MBR). Zapytanie do R-Tree odrzuca $99\%$ poligonów linii brzegowej na świecie.

2. **Narrow-Phase (Edge-Intersection):** Dla odfiltrowanych poligonów wewnątrz MBR wykonywany jest test przecięcia wektorowego (Sweep-line algorithm) dla linii trasy względem każdego segmentu (Edge) z uproszczonego (bufor $50\text{ m}$) wielokąta brzegowego S-57.

### 3.4. Korytarz H3 i zagnieżdżone siatki pogodowe

Wstępny korytarz routingu jest wyznaczany przez ograniczone wyszukiwanie A* po sąsiednich komórkach H3. Koszt krawędzi i heurystyka używają odległości geodezyjnej podzielonej przez maksymalną prędkość; połączenia przecinające przeszkody są odrzucane. Wynik zawiera ścieżkę komórek i poligon utworzony z ich unii. Limit rozwiniętych komórek chroni wyszukiwanie przed nieograniczonym zużyciem pamięci. Długości geograficzne poligonu są rozwijane względem długości startowej, aby zachować ciągłość przy antymeridianie.

Zagnieżdżone pola globalne i regionalne są łączone wagą logistyczną
$w = 1/(1 + e^{-d/s})$, gdzie $d$ jest podpisaną odległością od granicy regionu (dodatnia wewnątrz), a $s$ szerokością przejścia. Składowe wektorowe wiatru i prądu są interpolowane niezależnie; kierunek fal jest mieszany na okręgu jednostkowym, nie bezpośrednio jako kąt. Jeśli źródła mają sprzeczne kierunki fal dokładnie znoszące się przy danej wadze, zwracany jest błąd zamiast arbitralnego kierunku.

### 3.5. Optymalizacja odjazdu, ensemble i NMEA 2000

Optymalizacja odjazdu oblicza macierz tras dla unikalnych znaczników czasu
równolegle. Funkcja routingu jest współdzielona przez referencję, dzięki czemu
wywołujący może użyć tych samych mapowanych w pamięci siatek pogodowych.
Niebezpieczne trasy są pomijane, a najlepsza trasa jest wybierana po
deterministycznym koszcie końcowym (remis rozstrzyga wcześniejszy odjazd).

Ewaluacja ensemble uruchamia trasę dla każdego członka równolegle. Kosztem
ryzyka jest CVaR kosztów końcowych w najgorszym ogonie rozkładu, z obsługą
ułamkowej liczby próbek ogona; nieosiągalna trasa członka oznacza nieskończone
ryzyko. Kandydat jest akceptowany tylko wtedy, gdy jego CVaR nie przekracza
jawnie przekazanego limitu.

Monitor NMEA 2000 dekoduje PGN 129026 (COG/SOG), 130306 (wiatr) i 127250
(heading). Pozycję szacuje przez dead-reckoning z prawdziwego COG i SOG, a
odchylenie liczy do najbliższego odcinka planowanej trasy. Przekroczenie
konfigurowalnego progu XTE emituje zdarzenie z nowym punktem startowym i
niezmienionym celem; konsument strumienia może uruchomić ponowne obliczenie
trasy. Zdarzenie tworzy zaktualizowane żądanie routingu z nowym punktem startowym,
czasem i kursem, zachowując wskazany cel. Monitor zgłasza kolejne zdarzenie
dopiero po powrocie poniżej progu i ponownym jego przekroczeniu.

## 4. Model Danych i Pamięci (Data Architecture)

### 4.1. Struktury w pamięci (In-Memory)

Aby uniknąć zabójczego dla wydajności operacji odczytu blokowego z dysku (random I/O), silnik odrzuca klasyczne API `eccodes` w trakcie operacji na rzecz mapowania Zarr:

* **Format:** Spłaszczone tablice wielowymiarowe Zarr z chunkingiem ustawionym na profil czasowy, np. bloki wymiarów `[lat: 100, lon: 100, time: 240]`.

* **Zero-Copy:** Pliki Zarr są wyciągnięte do warstwy RAM węzła obliczeniowego z wykorzystaniem wywołania systemowego `mmap()`. Zwraca to bezpośredni wskaźnik do bloku pamięci zawierającego parametry $U, V$.

### 4.2. Schemat Bazy Danych

* **PostgreSQL / PostGIS (Persistent Storage):**

  ```
  CREATE TABLE no_go_zones (
      id UUID PRIMARY KEY,
      zone_type VARCHAR(50), -- np. 'TSS', 'ICE', 'REEF'
      geom GEOMETRY(Polygon, 4326),
      active_from TIMESTAMP,
      active_to TIMESTAMP
  );
  CREATE INDEX idx_nogo_geom ON no_go_zones USING GIST(geom);
  
  ```

* **Redis (Cache dla VPP i Stanu Zadań):**
  Klucz: `vpp:{vessel_id}` -> Wartość: Skompresowany JSON z tablicą TWS/TWA i mapą użycia żagli (Crossover chart).

## 5. Specyfikacja API (Interface Specification)

Wykorzystano środowisko Protocol Buffers (gRPC) do zminimalizowania narzutu sieciowego.

### 5.1. Definicja usługi (Protobuf)

```
syntax = "proto3";
package routing;

service RouteEngine {
  rpc CalculateRoute(RouteRequest) returns (stream RouteResponse);
  rpc ValidatePolar(PolarValidationRequest) returns (PolarValidationResponse);
}

message Coordinate {
  double latitude = 1;
  double longitude = 2;
}

message RouteRequest {
  string vessel_id = 1;
  Coordinate start = 2;
  Coordinate target = 3;
  int64 departure_time_unix = 4;
  double initial_heading_degrees = 5;
  optional int64 time_step_seconds = 6;
  optional double heading_step_degrees = 7;
  optional double max_tws_knots = 8;
  optional double max_wave_height_m = 9;
  optional uint32 max_steps = 10;
}

message PolarValidationRequest {
  string vessel_id = 1;
  string format = 2; // orc_json, csv_normalized, csv_matrix
  string content = 3;
}

message PolarValidationResponse {
  bool valid = 1;
  double maximum_speed_knots = 2;
  uint32 sail_count = 3;
}

message Waypoint {
  Coordinate point = 1;
  int64 time_unix = 2;
  double heading_degrees = 3;
  double course_degrees = 4;
  double sog_knots = 5;
  double fuel_remaining_l = 6;
  uint32 sail_index = 7;
  bool is_motoring = 8;
  double cumulative_cost_s = 9;
}

enum RouteEvent {
  ROUTE_EVENT_UNSPECIFIED = 0;
  ISOCHRONE_CHUNK = 1;
  FINAL_ROUTE = 2;
}

message RouteResponse {
  RouteEvent event = 1;
  uint32 isochrone_index = 2;
  repeated Coordinate isochrone_points = 3;
  repeated Waypoint waypoints = 4;
  string status = 5;
  bool warning_climatology_used = 6;
}

// RouteResponse is emitted repeatedly: ISOCHRONE_CHUNK as the search advances,
// followed by FINAL_ROUTE with the waypoints and terminal status.
```

### 5.2. Kontrakt Danych – JSON Schema (Dla REST Gateway)

*Endpoint:* `POST /v1/routes/calculate`

**Request Example:**

```
{
  "vessel_id": "pol_imoca60_foil_2026",
  "start": {"latitude": 47.5, "longitude": -5.1},
  "target": {"latitude": 25.0, "longitude": -77.3},
  "departure_time_unix": 1731517140,
  "initial_heading_degrees": 90.0,
  "time_step_seconds": 3600,
  "heading_step_degrees": 2.0,
  "max_tws_knots": 45.0,
  "max_wave_height_m": 6.5,
  "max_steps": 1000
}
```

Trasy krótsze niż 200 Mm są obliczane synchronicznie i zwracają wynik HTTP 200.
Dłuższe żądania zwracają HTTP 202 z `job_id`; ich stan, wynik, ostrzeżenia i
izochrony GeoJSON skompresowane gzip (Base64) są dostępne przez
`GET /v1/routes/{job_id}`. Stan zadania jest przechowywany w Redis przez 24 h.

`POST /v1/polars/validate` przyjmuje `vessel_id`, `format` i `content`; poprawny
polar ORC JSON lub CSV jest walidowany i zapisywany w Redis pod kluczem
`vpp:{vessel_id}`. Dozwolone wartości `format`: `orc_json`, `csv_normalized`,
`csv_matrix`.

### 5.3. Konfiguracja uruchomienia

Proces serwera wymaga `REDIS_URL`, `WEATHER_ZARR_PATH`, `POSTGIS_DATABASE_URL`
oraz ustawień routingu: `ROUTING_TIME_STEP_SECONDS`,
`ROUTING_HEADING_STEP_DEGREES`, `ROUTING_MAX_TWS_KNOTS`,
`ROUTING_MAX_WAVE_HEIGHT_M`, `ROUTING_MAX_STEPS`, `ROUTING_LEEWAY_COEFF`,
`ROUTING_HEEL_DEGREES`, `ROUTING_INITIAL_FUEL_L`, `ROUTING_TACK_PENALTY_S`,
`ROUTING_GYBE_PENALTY_S`, `ROUTING_SAIL_CHANGE_PENALTY_S`,
`ROUTING_FUEL_WEIGHT_S_PER_L`, `ROUTING_WAVE_WEIGHT_S_PER_M` i
`ROUTING_WAVE_COMFORT_HEIGHT_M`. Adresy nasłuchu można zmienić przez
`REST_ADDR` (domyślnie `0.0.0.0:8080`) i `GRPC_ADDR` (domyślnie
`0.0.0.0:50051`). Jednostki wejściowe API (węzły i stopnie) są konwertowane
do SI przed przekazaniem do silnika.

## 6. Wymagania Niefunkcjonalne (NFR) i Obsługa Błędów

### 6.1. Wydajność i Skalowalność (SLA)

* **Opóźnienie (Latency):**

  * Trasa lokalna (< 200 Mm): $P95 < 500 \text{ ms}$.

  * Trasa oceaniczna (> 3000 Mm): $P95 < 5 \text{ s}$ (czas do wygenerowania pierwszej pełnej trasy) z utrzymaniem ciągłego strumieniowania paczek (Chunked response) gRPC $< 500 \text{ ms}$ od żądania.

* **Zużycie RAM:** Jeden w pełni załadowany model pogodowy (np. GFS 0.25° na 10 dni, po dekompresji Zarr) zajmuje ok. 6-10 GB w pamięci operacyjnej per instancja. System korzysta z Shared Memory (shm), aby procesy workera na tej samej maszynie współdzieliły jeden wskaźnik do modelu pogody (unikając replikacji).

### 6.2. Obsługa Przypadków Brzegowych (Edge Cases)

* **Brak danych pogodowych na końcu trasy:** Jeżeli estymowany czas trasy przekracza horyzont pobranego GRIB-a (np. podróż > 15 dni), silnik po wyczerpaniu predykcji NWP przechodzi w tryb **Extrapolation fallback**. Wektory wiatru rzutowane są z uśrednionych danych historycznych (Pilot Charts / klimatologia) z flagą ostrzegawczą `WARNING_CLIMATOLOGY_USED` przesyłaną w odpowiedzi.

* **Brak bezpiecznej trasy (Sztorm totalny):** Jeżeli wszystkie węzły kandydujące zostaną odcięte (Pruning) na skutek uderzenia w limit $TWS_{max}$ (np. ogromny cyklon bez okna ucieczki), silnik zatrzymuje propagację. Zwracana jest chmura izochron aż do momentu kolapsu z błędem `ERR_NO_SAFE_ROUTE_FOUND` i rekomendacją punktu sztormowania (Heave-to).

* **Początek/Koniec na lądzie (Snapping):** Silnik weryfikuje $P_{start}$ i $P_{end}$ względem R-Tree. Jeżeli punkt uderza w ląd (np. wskutek błędu GPS lub niedokładności mapy), używany jest algorytm **k-NN (k-Nearest Neighbors)** na indeksie brzegowym celem znalezienia najbliższego punktu żeglownego. Tolerancja *snappingu* wynosi max. $2 \text{ Mm}$. Powyżej tej wartości zwracany jest błąd `ERR_INVALID_START_COORD`.