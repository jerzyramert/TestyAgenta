# Plan Implementacyjny: Moduł Pobierania i Przygotowania Map (`maps/`)

Niniejszy dokument definiuje architekturę, strukturę katalogów, sekwencję zadań (DAG) oraz protokół komunikacji dla modułu pobierania, weryfikacji i wstępnego przetwarzania danych kartograficznych (linia brzegowa, batymetria, strefy TSS) do lokalnego repozytorium `maps/`, skąd są one następnie ładowane do PostGIS / `R-Tree`.

> **Dyrektywa Operacyjna dla Agenta Kodującego (Single-File Protocol):**
> Niniejszy plik (`maps_ingestion_plan.md`) stanowi jedyne źródło prawdy oraz kanał komunikacji dla tego modułu. Agent **NIE TWORZY** osobnych plików `PROGRESS.md` ani `ANALYST_QUESTIONS.md` dla zadań `MAP-XX`. Wszelkie aktualizacje statusów zadań oraz pytania/założenia muszą być zapisywane bezpośrednio w **Sekcji 4** oraz **Sekcji 5** tego pliku.

---

## 1. Docelowa Struktura Katalogu `maps/`

Katalog `maps/` w głównym drzewie projektu jest podzielony na warstwę surową (nieśledzoną w Git — `.gitignore`) oraz przetworzoną, zoptymalizowaną pod indeksy przestrzenne:

```text
maps/
├── .gitkeep
├── manifest.json                 # Rejestr pobranych paczek, sum kontrolnych SHA256 i dat aktualizacji
├── raw/                          # Pobrane archiwa źródłowe (ignorowane przez Git)
│   ├── osm/                      # np. land-polygons-split-4326.zip
│   ├── gshhg/                    # np. gshhg-shp-2.3.7.zip
│   ├── natural_earth/            # np. ne_10m_land.zip (lekkie dane do testów CI/CD)
│   └── enc_s57/                  # Oficjalne komórki S-57 (*.000) z NOAA / EMODnet
└── processed/                    # Znormalizowane, uproszczone pliki gotowe dla PostGIS / Rust
    ├── coastlines_mvp.gpkg       # Połączone, uproszczone poligony lądu (bufor 50m, EPSG:4326)
    ├── bathymetry_pro.gpkg       # Warstwy głębokości (DEPARE) dla obliczeń UKC
    └── no_go_zones.gpkg          # Strefy separacji ruchu (TSS) i obszary zakazane
```

---

## 2. Architektura Modułu (`ingestion/maps_downloader.py`)

Moduł działa jako samodzielne narzędzie CLI w Pythonie (`>= 3.11`), współpracujące z istniejącym skryptem `ingestion/spatial_etl.py`:

* **Obsługiwane profile pobierania (`--profile`):**
  * `ci` (Lekki): Pobiera wyłącznie *Natural Earth 10m Land* (~5 MB) — używany w automatycznych testach jednostkowych i CI/CD.
  * `mvp` (Produkcyjny MVP): Pobiera *OSM Land Polygons (WGS84)* (~850 MB) dla pełnej, globalnej linii brzegowej.
  * `pro` (Pełny ENC): Pobiera paczki *NOAA ENC (S-57)* oraz batymetrię *GEBCO / EMODnet* dla zdefiniowanego obszaru (`--bbox min_lon,min_lat,max_lon,max_lat`).
* **Odporność i wydajność I/O:**
  * Pobieranie strumieniowe (`httpx` / `requests` z `stream=True`) z obsługą wznawiania (`HTTP Range` headers) przy zerwaniu połączenia.
  * Weryfikacja integralności plików za pomocą sum kontrolnych `SHA256` zapisywanych w `maps/manifest.json` (pomijanie ponownego pobierania, jeśli lokalny plik jest aktualny).
  * Bezpieczne rozpakowywanie archiwów `.zip` z ochroną przed atakiem *Zip Slip*.

---

## 3. Specyfikacja Zadań Technicznych (WBS / DAG)

Wszystkie zadania należy zrealizować z zachowaniem standardów jakości z `AGENTS.md` (`ruff check . && mypy . && pytest`).

### `MAP-01`: Konfiguracja struktury katalogów i `.gitignore`
* **Zależności:** Brak
* **Pliki:** `.gitignore`, `maps/.gitkeep`, `ingestion/pyproject.toml`
* **Zakres prac:**
  * Dodać reguły do `.gitignore` wykluczające `maps/raw/**` oraz `maps/processed/**` (z wyjątkiem `maps/.gitkeep` i `maps/manifest.json`), aby nie zaśmiecać repozytorium plikami gigabajtowymi.
  * Dodać zależności do `ingestion/pyproject.toml`: `httpx`, `tqdm` (pasek postępu CLI), `pyogrio` / `geopandas`, `shapely`.
* **Warunek przejścia (DoD):** `git status` nie śledzi plików binarnych w `maps/raw/`, a środowisko Pythona poprawnie importuje wymagane biblioteki.

### `MAP-02`: Implementacja silnika pobierania i weryfikacji (`maps_downloader.py`)
* **Zależności:** `MAP-01`
* **Pliki:** `ingestion/maps_downloader.py`, `ingestion/sources_registry.py`
* **Zakres prac:**
  * Zdefiniować rejestr źródeł (`sources_registry.py`) zawierający bezpośrednie adresy URL dla profili `ci` (Natural Earth 10m), `mvp` (OSM Land Polygons WGS84), `gshhg` (NOAA GSHHG) oraz endpointy katalogów NOAA ENC S-57.
  * Zaimplementować pobieranie strumieniowe z paskiem postępu, automatycznym ponawianiem (exponential backoff przy błędach HTTP `5xx` / timeout) oraz weryfikacją `SHA256`.
  * Zaimplementować aktualizację pliku `maps/manifest.json` (data pobrania, wersja źródła, rozmiar, hash pliku).
* **Warunek przejścia (DoD):** Uruchomienie `python -m ingestion.maps_downloader --profile ci` pobiera paczkę do `maps/raw/natural_earth/`, rozpakowuje ją i zapisuje poprawny wpis w `maps/manifest.json`. Ponowne uruchomienie pomija pobieranie (`Cache hit`).

### `MAP-03`: Wstępne przetwarzanie geometrii do `maps/processed/`
* **Zależności:** `MAP-02`
* **Pliki:** `ingestion/maps_preprocessor.py`
* **Zakres prac:**
  * Przetworzenie surowych plików `.shp` / `.gpkg` z `maps/raw/` do ustandaryzowanego formatu GeoPackage w `maps/processed/coastlines_mvp.gpkg` (układ `EPSG:4326`).
  * Nałożenie bufora bezpieczeństwa $50\text{ m}$ (`0.00045` stopnia lub transformacja przez lokalny układ metryczny) zgodnie z wymogami `FR-004` / `STEP-03`.
  * Uproszczenie wierzchołków algorytmem Douglasa-Peuckera (`tolerance = 0.0005`) oraz podział gigantycznych poligonów kontynentalnych na mniejsze kafelki (maks. 256 wierzchołków na poligon) pod indeks `R-Tree`.
* **Warunek przejścia (DoD):** Wygenerowany plik `maps/processed/coastlines_mvp.gpkg` przechodzi walidację topologiczną (`geom.is_valid == True` dla 100% rekordów).

### `MAP-04`: Parser komórek S-57 (`.000`) dla Fazy Pro (Batymetria i TSS)
* **Zależności:** `MAP-02`
* **Pliki:** `ingestion/s57_extractor.py`
* **Zakres prac:**
  * Wykorzystanie `pyogrio` / `GDAL` do ekstrakcji kluczowych warstw z plików IHO S-57 (`maps/raw/enc_s57/*.000`):
    * `LNDARE` $\rightarrow$ dołączenie do poligonów lądu.
    * `DEPARE` $\rightarrow$ zapis do `maps/processed/bathymetry_pro.gpkg` z zachowaniem atrybutów `DRVAL1` (minimalna głębokość) i `DRVAL2` (maksymalna głębokość) dla wzoru UKC / Squat Barrassa.
    * `TSSLPT` / `TSSBND` $\rightarrow$ zapis do `maps/processed/no_go_zones.gpkg` jako strefy wyłączone.
* **Warunek przejścia (DoD):** Test na przykładowej komórce NOAA ENC S-57 poprawnie eksportuje izobaty i strefy TSS do odpowiednich plików `.gpkg`.

### `MAP-05`: Integracja `maps/processed/` z `spatial_etl.py` i testami CI
* **Zależności:** `MAP-03`, `MAP-04`
* **Pliki:** `ingestion/spatial_etl.py`, `ingestion/tests/test_maps_downloader.py`
* **Zakres prac:**
  * Przepięcie `ingestion/spatial_etl.py`, aby domyślnie czytał gotowe warstwy z `maps/processed/*.gpkg` i ładował je do tabel PostGIS (`coastlines`, `bathymetry`, `no_go_zones`) z wykorzystaniem `ST_Subdivide(geom, 256)`.
  * Napisanie testów jednostkowych i integracyjnych (`pytest`) z użyciem lokalnego mocka serwera HTTP oraz profilu `ci`.
* **Warunek przejścia (DoD):** `ruff check . && mypy . && pytest ingestion/tests/` przechodzi w 100%, a dane z `maps/processed/` są poprawnie odpytywane przez indeks `R-Tree` w Rust.

---

## 4. Kolejka Realizacji i Status Zadań (Execution Tracker)

**Instrukcja aktualizacji dla Agenta:**
1. Przed rozpoczęciem zadania zmień jego status na `[IN_PROGRESS]`.
2. Po spełnieniu wszystkich warunków DoD oraz przejściu `ruff check . && mypy . && pytest ingestion/tests/`, zmień checkbox na `[x]`, ustaw status `[DONE]` i uzupełnij pole `Notatki / Artefakty`.
3. W przypadku napotkania niejasności lub blokady wpisz pytanie w **Sekcji 5**, oznacz zadanie jako `[BLOCKED by Q-MAP-XX]` i przejdź do innego niezależnego zadania z listy.

- [ ] **MAP-01**: Konfiguracja struktury katalogów i `.gitignore`
  - **Status:** `[TODO]`
  - **Zależności:** Brak
  - **Notatki / Artefakty:** *Oczekuje na realizację*

- [ ] **MAP-02**: Implementacja silnika pobierania i weryfikacji (`maps_downloader.py`)
  - **Status:** `[TODO]`
  - **Zależności:** `MAP-01`
  - **Notatki / Artefakty:** *Oczekuje na realizację*

- [ ] **MAP-03**: Wstępne przetwarzanie geometrii do `maps/processed/`
  - **Status:** `[TODO]`
  - **Zależności:** `MAP-02`
  - **Notatki / Artefakty:** *Oczekuje na realizację*

- [ ] **MAP-04**: Parser komórek S-57 (`.000`) dla Fazy Pro (Batymetria i TSS)
  - **Status:** `[TODO]`
  - **Zależności:** `MAP-02`
  - **Notatki / Artefakty:** *Oczekuje na realizację*

- [ ] **MAP-05**: Integracja `maps/processed/` z `spatial_etl.py` i testami CI
  - **Status:** `[TODO]`
  - **Zależności:** `MAP-03`, `MAP-04`
  - **Notatki / Artefakty:** *Oczekuje na realizację*

---

## 5. Strefa Komunikacji i Pytań Agenta (Agent Q&A / Escalation Log)

**Zasady korzystania ze strefy Q&A:**
* **Dla Agenta Kodującego:** Jeśli w trakcie realizacji `MAP-01` – `MAP-05` natrafisz na brakujący adres URL źródła, niejednoznaczność dotyczącą tolerancji uproszczenia geometrii, układu współrzędnych lub sposobu obsługi brakujących warstw S-57, **dopisz nowy wpis na końcu tej sekcji** zgodnie z poniższym szablonem. Jeśli możesz przyjąć bezpieczne założenie tymczasowe, oznacz status jako `[OPEN - ASSUMED]` i dodaj komentarz `# ASSUMPTION [Q-MAP-XX]: ...` w kodzie.
* **Dla Zespołu / Użytkownika:** Wpisz odpowiedź w polu **Odpowiedź / Decyzja**, zmień status na `[RESOLVED]` i usuń blokadę `[BLOCKED]` z odpowiedniego zadania w Sekcji 4.

### Szablon wpisu (kopiuj przy dodawaniu nowego pytania):
```markdown
#### [Q-MAP-XX] <Krótki tytuł pytania>
- **Status:** `[OPEN - BLOCKING]` | `[OPEN - ASSUMED]` | `[RESOLVED]`
- **Dotyczy zadania:** `MAP-XX` | **Plik:** `<ścieżka/do/pliku.py>`
- **Pytanie / Opis problemu:**
  <Treść pytania lub opis napotkanej niejednoznaczności technicznej>
- **Tymczasowe założenie Agenta (Fallback):**
  <Opis przyjętego obejścia lub "Brak – zadanie zablokowane">
- **Odpowiedź / Decyzja (Wypełnia człowiek):**
  <Tutaj wpisz odpowiedź>
```

### Aktywne i historyczne wpisy Q&A:

*(Brak otwartych pytań — strefa gotowa na wpisy agenta oraz użytkownika)*