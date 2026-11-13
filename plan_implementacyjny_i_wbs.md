# Plan Implementacyjny i WBS (Engineering Execution Plan)

## Silnik Routingu Pogodowego Jachtów Żaglowych

## 1. Strategia Wdrożenia i Kamienie Milowe (Milestones)

Proces wdrożenia podzielono na 6 etapów, gwarantując ciągłe dostarczanie wartości i minimalizację ryzyka architektonicznego.

* **M0: Fundamenty i Infrastruktura (T0 + 2 tygodnie)**

  * **Cel:** Konfiguracja repozytoriów, CI/CD, szkieletu usług (Rust gRPC, Python Ingest), postawienie PostGIS oraz Redis.

  * **Deliverable:** Pusty endpoint gRPC przyjmujący zapytanie i zwracający mockowaną odpowiedź; pipeline CI budujący kontenery.

* **M1: Walking Skeleton (T0 + 5 tygodni)**

  * **Cel:** Najprostszy algorytm szukający drogi z pominięciem lądu (płaskie morze, stały wiatr, brak zmiennej pogody).

  * **Deliverable:** Silnik generuje trasę z $A$ do $B$, omijając proste poligony lądowe S-57 (R-Tree in-memory). Prosta fizyka bazująca na jednym wektorze wiatru z mocka.

* **M2: Fizyka Jachtu i Dane Pogodowe (T0 + 8 tygodni)**

  * **Cel:** Asymilacja rzeczywistych danych (FR-001) i wdrożenie modelu jachtu (FR-004).

  * **Deliverable:** Działający pipeline Python GRIB2 -> Zarr na S3. Rust odczytuje Zarr przez `mmap`. Poprawne wyliczanie STW/SOG na podstawie interpolacji 4D (FR-002).

* **M3: Pełny Silnik MVP (T0 + 11 tygodni)**

  * **Cel:** Działająca metoda Izochron, kary za manewry (FR-005), obsługa stref wyłączonych (No-Go).

  * **Deliverable:** Algorytm z powodzeniem trasuje węzły za pomocą izochron z "bearing buckets" (FR-007). Aktywny "No-Sail Zone Tapering" (FR-008). Pełna gotowość fazy 1 (MVP).

* **M4: Funkcje Pro – Część 1 (T0 + 15 tygodni)**

  * **Cel:** Optymalizacje przestrzenne i zaawansowana fizyka morza.

  * **Deliverable:** Zaimplementowany korytarz heurystyczny A\* (FR-009) zapobiegający problemom w archipelagach. Wdrożona redukcja uciągu przez opór falowy (FR-006).

* **M5: Funkcje Pro – Część 2 (T0 + 19 tygodni)**

  * **Cel:** Systemy stochastyczne, Ensemble Routing, operacje czasu rzeczywistego.

  * **Deliverable:** Dynamic Re-routing z szyny NMEA (FR-010). Grid Blending modeli numerycznych (FR-003).

## 2. Szczegółowy Wykaz Zadań (Work Breakdown Structure - WBS)

### Epik 1: Infrastruktura i Dane Przestrzenne (M0-M1)

| ID | Nazwa Zadania | Powiązanie | Rola | Rozmiar | Zależności | Definition of Done (DoD) | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **TASK-101** | Szkielet gRPC i Protobuf | Architektura | Backend | XS | Brak | Wygenerowane stuby gRPC (Rust). Endpoint `CalculateRoute` przyjmuje request i oddaje pusty 200 OK. Unit testy protokołu. | 
| **TASK-102** | Baza PostGIS & Zasilanie S-57 | Architektura | GIS | M | Brak | Skrypt Python wczytuje mapy S-57/GSHHG do PostGIS. Geometrie uproszczone (bufor 50m). Utworzony indeks GIST. | 
| **TASK-103** | In-memory R-Tree w Rust | Architektura | Core Algo | L | TASK-102 | Worker (Rust) przy starcie ładuje MBR-y z PostGIS. Funkcja `intersects_land(lat1, lon1, lat2, lon2)` działa < 1ms. Napisane benchmarki w `criterion.rs`. | 

### Epik 2: Asymilacja Pogody (M1-M2)

| ID | Nazwa Zadania | Powiązanie | Rola | Rozmiar | Zależności | Definition of Done (DoD) | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **TASK-201** | Pipeline GRIB2 -> Zarr (Python) | FR-001 | Data | M | Brak | Skrypt Airflow/Cron pobiera GRIB2 (wiatr, prądy), dekompresuje AEC/JPEG2000 i zapisuje paczki Zarr do S3 z poprawnym chunkingiem czasowym. | 
| **TASK-202** | Zero-copy mmap Zarr w Rust | FR-001, Arch | Core Algo | L | TASK-201 | Rust odczytuje Zarr z dysku przez `mmap`. Napisany wrapper dostępu do składowych U/V. Test alokacji: zużycie RAM nie skacze o wielkość pliku. | 
| **TASK-203** | Matematyka interpolacji 4D | FR-002 | Core Algo | S | TASK-202 | Zaimplementowana interpolacja trójliniowa. Testy parametryczne na granicy datownika (180°) i biegunach przechodzą bez błędu zawijania (wrap-around). | 

### Epik 3: Model Fizyki Jachtu (M1-M2)

| ID | Nazwa Zadania | Powiązanie | Rola | Rozmiar | Zależności | Definition of Done (DoD) | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **TASK-301** | Parser plików VPP i Cache | FR-004 | Backend | S | Brak | Rust parsuje VPP/CSV do zoptymalizowanej struktury w pamięci (tablica 2D interpolowana). Połączenie z Redisem działa. | 
| **TASK-302** | Składanie wektorów SOG/COG | FR-004 | Core Algo | M | TASK-301 | Funkcja łączy STW, wiatr $U/V$, prąd $U/V$ i Leeway. Zawiera testy z wymuszonym stałym prądem poprzecznym 2kt (Golden Tests). | 
| **TASK-303** | Silnik Motor-Sailing i Kary | FR-005 | Core Algo | S | TASK-302 | Pętla ewaluuje prędkość. Jeśli $STW < STW_{min}$, dodaje koszt paliwa i zamienia wektor. Zaimplementowane kary czasowe za zwrot (tack/gybe penalty). | 

### Epik 4: Silnik Izochron (M3)

| ID | Nazwa Zadania | Powiązanie | Rola | Rozmiar | Zależności | Definition of Done (DoD) | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **TASK-401** | Pętla główna Izochron | FR-007 | Core Algo | XL | TASK-103, 302 | Propagacja siatki w czasie $\Delta t$. (Ze względu na rozmiar XL, zadanie do podziału na subtaski: generacja węzłów kandydujących, ocena fizyki, pruning). | 
| **TASK-402** | Filtracja "Bearing Buckets" | FR-007 | Core Algo | M | TASK-401 | W każdym kroku izochrony chmura jest sortowana do koszyków kątowych (np. co 1° względem celu). Pozostaje węzeł z maksymalnym VMC (Velocity Made Good). | 
| **TASK-403** | No-Sail Zone Tapering | FR-008 | Core Algo | S | TASK-401 | Wektor wiatru w strefie martwej jest redukowany rampą do 0. Testy zapobiegające bezpośrednim rzutom w wiatr. | 

### Epik 5: Funkcje Pro i Integracja (M4-M5)

| ID | Nazwa Zadania | Powiązanie | Rola | Rozmiar | Zależności | Definition of Done (DoD) | 
| ----- | ----- | ----- | ----- | ----- | ----- | ----- | 
| **TASK-501** | Heurystyka A\* (Korytarz) | FR-009 | Core Algo | L | TASK-103 | Moduł wyznacza rygorystyczny graf 2D siatką H3, omijając lądy i generuje "poligon korytarza", w którym mogą propagować się Izochrony (wąskie gardła). | 
| **TASK-502** | Dynamiczne zapytania NMEA | FR-010 | API/Backend | M | TASK-101, 401 | Endpoint akceptuje websocketowy strumień NMEA. Trigger re-routingu uruchamia się, gdy $XTE > limit$. | 

## 3. Graf Zależności i Ścieżka Krytyczna (Critical Path)

Poniższy diagram ilustruje przepływ zadań. Główna oś ryzyka (Ścieżka Krytyczna) dotyczy modułów Core Algo (Zarr -> Interpolacja -> Izochrony).

```
gantt
    title Roadmapa Implementacji - Faza MVP (M0-M3)
    dateFormat  YYYY-MM-DD
    axisFormat  %W
    
    section Infrastruktura & GIS
    TASK-101 API gRPC              :done, t101, 2026-11-16, 3d
    TASK-102 Baza PostGIS & S-57   :active, t102, 2026-11-16, 5d
    TASK-103 R-Tree Rust (CRITICAL):crit, active, t103, after t102, 7d
    
    section Dane Pogodowe
    TASK-201 Pipeline GRIB->Zarr   :t201, 2026-11-16, 7d
    TASK-202 mmap Zarr (CRITICAL)  :crit, t202, after t201, 7d
    TASK-203 Interpolacja 4D       :t203, after t202, 4d
    
    section Fizyka Jachtu
    TASK-301 Parser VPP            :t301, 2026-11-20, 3d
    TASK-302 Składanie Wektorów    :t302, after t301, 5d
    TASK-303 Motor-Sailing         :t303, after t302, 2d
    
    section Silnik Izochron
    TASK-401 Pętla Główna (CRITICAL):crit, t401, after t103 t203 t302, 12d
    TASK-402 Bearing Buckets       :crit, t402, after t401, 5d
    TASK-403 No-Sail Zone          :t403, after t401, 3d

```

**Analiza Ścieżki Krytycznej:**

1. **TASK-103 (In-memory R-Tree):** Omijanie lądu musi być gotowe, zanim ruszy główna pętla izochron. Optymalizacja broad-phase R-Tree narzuca ramy wydajnościowe całemu algorytmowi.

2. **TASK-202 (mmap Zarr):** Dopóki silnik Rust nie potrafi odpytywać pogody w pamięci $O(1)$, nie można zintegrować w pełni warstwy modelu jachtu.

3. **TASK-401 i TASK-402 (Izochrony & Bucketing):** Fundamentalny "mięsień" systemu. Złożoność algorytmiczna w tym miejscu determinuje czy system odpowie w obiecanych $<5\text{s}$ (SLA).

## 4. Strategia Testowania i Walidacji Matematycznej (QA & Verification Plan)

Ze względu na fizyczny charakter symulacji, system nie będzie posiadał jedynie testów REST API, ale wdroży dedykowany aparat weryfikacyjny:

1. **Syntetyczne Scenariusze (Golden Test Cases):**

   * *Test Dryfu w Prądzie:* $TWS = 0$, $STW = 5\text{kt}$, silnik załączony. Prąd poprzeczny (Beam Current) stały $2\text{kt}$. Asertywna weryfikacja czy COG odchyla się o poprawny kąt arcusa tangensa od linii statku.

   * *Test Archipelagu (Omijanie przeszkód):* Syntetyczna mapa $10\times10\text{ Mm}$ z wrysowaną idealnie okrągłą wyspą. Wiatr prosto od celu (Dead upwind). Asertywna weryfikacja, czy wężykowanie (Tacking) jachtu płynnie opływa wyspę bez "przyklejania się" do MBR poligonu.

2. **Benchmarki Wydajnościowe (CI/CD):**

   * Wykorzystanie frameworka `criterion.rs` uruchamianego w pipeline GitHub Actions / GitLab CI.

   * *Mikro-benchmarki:* Pomiar czasu dla $100\ 000$ zapytań o wartość meteorologiczną `get_4d_point()` (limit: $< 5\text{ ms}$).

   * *Makro-benchmarki:* Odpalenie trasy referencyjnej *Newport -> Bermuda* ze stałym VPP i porównanie czasu trwania iteracji oraz profili zużycia pamięci `valgrind / massif` względem gałęzi `main`.

3. **Walidacja Krzyżowa (Cross-Validation):**

   * Baza `pytest` uruchamiająca CLI naszego silnika oraz natywnej binarnej wersji narzędzia referencyjnego (np. *Weather Routing Plugin* dla OpenCPN) podając ten sam plik GRIB i polary.

   * Automatyczne porównanie czasu trwania, zużycia paliwa i dystansu. Akceptowalna delta wynosi $< 2\%$. Wyprowadzony raport graficzny (porównanie śladów GPX nałożonych na siebie).

## 5. Rejestr Ryzyk Technicznych i Plan Mitygacji (Risk Register)

| ID | Zidentyfikowane Ryzyko | Potencjalny Wpływ (Impact) | Mitygacja Architektoniczna | 
| ----- | ----- | ----- | ----- | 
| **R-01** | Eksplozja kombinatoryczna węzłów Izochron. Wąskie przejścia (np. cieśniny duńskie) zablokują algorytm. | OOM (Out of Memory) crash, nie spełnienie SLA czasu (timeout). | Konieczność implementacji "Bearing Buckets" z bardzo agresywnym *pruningiem* węzłów gorszych oraz uruchomienie heurystyki A\* (TASK-501) jako sztywnego korytarza dla obszarów śródlądowych. | 
| **R-02** | Wycieki pamięci / RAM Exhaustion na węzłach (Worker) przy plikach GRIB obejmujących cały glob dla prognoz 15-dniowych. | Zawieszenie węzła, kaskadowa degradacja kolejki zadań (Redis). | Całkowita rezygnacja z biblioteki `eccodes` w rdzeniu. Transformacja offline (Python) do Zarr i agresywne użycie `mmap()` do współdzielenia odczytów RAM (Shared Memory) między wątkami. | 
| **R-03** | Niespójność siatek przestrzenno-czasowych. Model fali $0.5^\circ$, model wiatru $0.25^\circ$, model prądu 3-godzinny. | Rzucanie wyjątkami z interpolatora, błędy składania wektorów (NaN propagation). | Zbudowanie ujednoliconych proxy w Rust. Rozdzielenie interpolatorów – najpierw mapujemy zmienną X do ułamkowych $Lat/Lon/T$, dopiera potem skalar wchodzi do równania $SOG$. | 
| **R-04** | Zjawisko "Wrap-Around" na południku 180° i na polaryzacji. | Trasy nagle przecinające całą kulę ziemską; jacht "zawraca" by złapać sztuczną średnią kierunku. | Ścisłe wdrożenie operacji na wektorach bazowych U i V zamiast matematyki na kątach. Ortotroma rzutowana z matematycznym modulo $2\pi$ (TASK-203). Obowiązkowe Golden Testy na Pacyfiku. | 
| **R-05** | Wąskie gardło przy zasilaniu PostGIS danymi brzegowymi i snappingu do wody (Start z "lądu"). | Klienci (np. aplikacja mobilna) podają współrzędne z nabrzeża (molo). Silnik odrzuca 30% żądań jako niedozwolone (kolizja). | Wdrożenie algorytmu Snap-To-Water: odpytanie k-NN (k-Nearest Neighbors) wokół punktu startu i automatyczne przesunięcie markera na najbliższy obszar niekolidujący w promieniu tolerancji 2 Mm. | 
