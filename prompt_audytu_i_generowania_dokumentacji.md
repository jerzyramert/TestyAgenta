# Rola i Cel Zadania
Wciel się w rolę Principal Staff Engineera oraz Technical Documentation Architecta. Twoim zadaniem jest przeprowadzenie **kompleksowego audytu weryfikacyjnego** zaimplementowanego w repozytorium kodu źródłowego względem dokumentacji projektowej (`AGENTS.md`, `weather_routing_engine_sdd.md`, `execution_plan.md`, `PROGRESS.md`), a następnie wygenerowanie **kompletnej, produkcyjnej dokumentacji technicznej projektu w katalogu `/doc`**.

---

# Faza 1: Rygorystyczny Audyt Kodu i Weryfikacja Zgodności (Verification Audit)

Zanim napiszesz jakiekolwiek pliki dokumentacji w `/doc`, musisz wykonać pełną inspekcję repozytorium i zweryfikować spełnienie wszystkich wymagań:

1. **Inspekcja plików bazowych:**
   - Przeczytaj w całości: `AGENTS.md`, `weather_routing_engine_sdd.md`, `execution_plan.md`, `PROGRESS.md` oraz (jeśli istnieje) `ANALYST_QUESTIONS.md`.
2. **Audyt Statyczny i Testowy (Terminal):**
   - Uruchom pełny zestaw komend weryfikacyjnych:
     - Rust: `cargo fmt --check && cargo clippy -- -D warnings && cargo test --workspace`
     - Python (ETL): `ruff check . && mypy . && pytest`
   - Zweryfikuj, czy wszystkie **Golden Test Cases** z Sekcji 6 pliku `AGENTS.md` (`test_upwind_tacking`, `test_current_drift`, `test_island_avoidance`, `test_antimeridian`, `test_edge_cases`) istnieją i przechodzą w 100%.
3. **Audyt Architektoniczny i Jakościowy (Code Review):**
   - Przeszukaj katalog `/src` pod kątem zakazanych konstrukcji (`unwrap()`, `expect()`, `todo!()`, `unimplemented!()`, `// TODO`).
   - Zweryfikuj przestrzeganie **jednostek SI** (`m/s`, `rad`, `m`, `WGS84`, `UTC Unix timestamp`) w rdzeniu obliczeniowym (`physics`, `weather`, `spatial`, `core`) oraz poprawność konwersji na granicach API (`src/api/`).
   - Zweryfikuj obecność komentarzy `// ASSUMPTION [Q-XXX]` w kodzie i porównaj je z wpisami w `ANALYST_QUESTIONS.md`.
   - Sprawdź, czy każde wymaganie funkcjonalne (`FR-001` – `FR-012`) oraz niefunkcjonalne (`NFR`, SLA, obsługa błędów) ma odzwierciedlenie w działającym kodzie.
4. **Wygenerowanie Raportu z Audytu (`/doc/AUDIT_REPORT.md`):**
   - Zapisz wynik weryfikacji w pliku `/doc/AUDIT_REPORT.md` zawierającym:
     - Macierz pokrycia wymagań (Requirements Traceability Matrix: `FR-XXX` / `STEP-XX` -> pliki źródłowe -> testy weryfikujące -> status `[PASS / FAIL / PARTIAL]`).
     - Wykaz wszystkich wykrytych rozbieżności, braków lub długu technicznego.
     - Podsumowanie otwartych pytań z `ANALYST_QUESTIONS.md` i przyjętych założeń (`ASSUMPTION`).
   - **UWAGA:** Jeśli w trakcie audytu wykryjesz jakiekolwiek błędy kompilacji, niespełnione kryteria akceptacji lub brakujące testy z listy `AGENTS.md` – **najpierw napraw kod i testy**, aż wszystko będzie w 100% zielone, a dopiero potem przejdź do Fazy 2.

---

# Faza 2: Generowanie Kompletnej Dokumentacji Kodu w Katalogu `/doc`

Po zakończeniu audytu utwórz katalog `/doc` i wygeneruj w nim kompletny zestaw dokumentacji technicznej odzwierciedlającej **rzeczywisty stan zaimplementowanego kodu** (nie kopiuj ślepo specyfikacji – opisuj faktyczne struktury, sygnatury funkcji, schematy i konfiguracje istniejące w repozytorium):

1. **`/doc/README.md` (Spis Treści i Przegląd Systemu)**
   - Architektura wysokopoziomowa (zaktualizowany diagram Mermaid.js odzwierciedlający faktyczne moduły i zależności w kodzie).
   - Przewodnik po strukturze katalogów i zasadach przepływu zależności.
   - Indeks wszystkich dokumentów w katalogu `/doc`.

2. **`/doc/01_SETUP_AND_OPERATIONS.md` (Uruchomienie, Konfiguracja i Infrastruktura)**
   - Wymagania środowiskowe (wersje kompilatora Rust, Python, PostgreSQL/PostGIS, Redis, S3/MinIO).
   - Zmienne środowiskowe (`.env`), konfiguracja pamięci współdzielonej (`mmap` / `shm`) i indeksów przestrzennych.
   - Instrukcja krok po kroku: migracje bazy danych, uruchomienie potoków ETL (pogoda GRIB2->Zarr oraz mapy S-57->PostGIS), start serwera gRPC/REST oraz uruchamianie testów i benchmarków (`criterion`).

3. **`/doc/02_CORE_ALGORITHMS_AND_MATH.md` (Dokumentacja Algorytmów i Modelu Fizycznego)**
   - Szczegółowy opis zaimplementowanej matematyki (kinematyka wektorowa `STW` + `Leeway` + `Current` = `SOG`/`COG`, interpolacja VPP, opór falowy ISO 15016/Kwon, dynamiczny UKC/Squat Barrassa) z odniesieniami do konkretnych plików i funkcji w `src/physics/`.
   - Opis działania głównej pętli propagacji izochron (`src/core/isochrone.rs`), redukcji przez *Bearing Buckets*, korytarza heurystycznego A* (`H3`) oraz analizy wiązkowej (Ensemble / CVaR).
   - Opis silnika przestrzennego (`src/spatial/`): geodezja sferyczna, korekta antypołudnika, dwufazowa detekcja kolizji R-Tree (broad-phase + narrow-phase) oraz algorytm *Snap-To-Water*.

4. **`/doc/03_DATA_AND_STORAGE.md` (Architektura Danych, Zarr i Baza Przestrzenna)**
   - Schemat bazy danych PostgreSQL/PostGIS (tabele, indeksy GIST, zapytania przestrzenne).
   - Struktura plików Zarr (wymiary chunków, układ pamięci, zero-copy `mmap` w Rust, interpolacja trójliniowa 4D).
   - Struktura kluczy i cache'owania w Redis (biegunowe VPP, stan zadań asynchronicznych).

5. **`/doc/04_API_REFERENCE.md` (Dokumentacja Interfejsów gRPC, REST i NMEA)**
   - Pełna dokumentacja usług gRPC (`proto/routing.proto`) wraz z opisem strumieniowania odpowiedzi.
   - Pełna dokumentacja endpointów REST Gateway (`POST /v1/routes/calculate`, `GET /v1/routes/{job_id}`, `POST /v1/polars/validate`) z realistycznymi przykładami `Request` / `Response` JSON.
   - Specyfikacja strumienia telemetrii NMEA 2000 (obsługiwane PGN) i logiki wyzwalania *Dynamic Re-routing*.
   - Katalog kodów błędów domenowych (`DomainError`) i flag ostrzegawczych (`WARNING_CLIMATOLOGY_USED`, `ERR_NO_SAFE_ROUTE_FOUND`, `ERR_INVALID_START_COORD`).

6. **`/doc/05_TESTING_AND_BENCHMARKS.md` (Strategia QA, Golden Tests i Wydajność)**
   - Dokumentacja wszystkich *Golden Test Cases* (założenia fizyczne, dane wejściowe, tolerancje numeryczne).
   - Wyniki uruchomionych benchmarków `criterion.rs` (czasy odpowiedzi dla tras krótkich i oceanicznych, zużycie pamięci RAM) oraz analiza zgodności z wymaganiami SLA.

---

# Zasady Wykonania
- **Prawda w kodzie:** Dokumentacja w `/doc` musi w 100% odpowiadać faktycznym nazwom struktur, funkcji, plików i endpointów w repozytorium.
- **Kompletność:** Nie używaj skrótów myślowych typu „analogicznie dla pozostałych funkcji”. Każdy publiczny moduł i endpoint musi zostać udokumentowany.
- **Formatowanie:** Stosuj przejrzysty Markdown, bloki kodu z podświetlaniem składni, tabele oraz diagramy Mermaid.js. Wszystkie wzory matematyczne zapisuj w formacie LaTeX (`$` / `$$`).
```

Ten prompt jest gotowy do bezpośredniego wklejenia w czacie agenta (np. w Cursorze lub VS Code) – wymusi na nim najpierw twardą weryfikację kodu i naprawę ewentualnych braków, a dopiero na końcu wygenerowanie spójnej, wieloplikowej dokumentacji w `/doc`.