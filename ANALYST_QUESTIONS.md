## [Q-001] Tolerancja upraszczania geometrii wybrzeży
- **Status:** `[CLOSED]`
- **Kontekst:** `STEP-03` | Moduł: `spatial_etl` | Plik: `ingestion/spatial_etl.py`
- **Opis problemu / brakującej wiedzy:**
  Jaka maksymalna tolerancja upraszczania w metrach jest dopuszczalna dla geometrii wybrzeży, aby zachować wąskie przesmyki i małe wyspy istotne dla routingu?
- **Odpowiedź analityka:**
  Przyjąć tolerancję upraszczania `100 m` jako wartość domyślną.
- **Zastosowanie:**
  Wartość domyślna w `ingestion/spatial_etl.py` została ustawiona na `100 m`; parametr `--simplify-tolerance-m` pozwala ją jawnie nadpisać.

## [Q-002] Schematy wejściowe polaru VPP
- **Status:** `[CLOSED]`
- **Kontekst:** `STEP-05` | Moduł: `physics::polar` | Plik: `src/physics/polar.rs`
- **Odpowiedź analityka:** Pełna specyfikacja znajduje się w `polar.md`: jednostki TWS/BS, kąty i macierz `.pol`, schemat ORC JSON, znormalizowany i jawnie wybierany macierzowy CSV, invalid-cell handling, obliczanie VMG/crossoverów, kwadratowe skalowanie poniżej minimalnego TWS i strefa no-sail.
- **Zastosowane obejście / tymczasowe założenie (Fallback):**
  Brak. Implementacja używa interpolacji liniowej VMG między kolumnami TWS oraz liniowego taperu downwind do 180° zgodnie z regułą rampy w `AGENT.MD`.
- **Zastosowanie:** Parsery `.pol`, ORC JSON, CSV (znormalizowany i jawnie wybrany macierzowy), interpolacja, SI conversion, VMG/crossover tables i Redis key `vpp:{vessel_id}` zaimplementowano i zweryfikowano testami.

## [Q-003] Opór falowy wyłączony z MVP
- **Status:** `[CLOSED]`
- **Kontekst:** `STEP-12` | Moduł: `physics::resistance` | Plik: `BYPASS_FUTURES.md`
- **Odpowiedź analityka:**
  W wersji MVP pomijamy opór falowy.
- **Zastosowanie:**
  Redukcja STW z tytułu fal nie będzie stosowana w MVP; wymagania przyszłej implementacji zapisano w `BYPASS_FUTURES.md`.

## [Q-004] Interpretacja progu 5 m i profil jednostki dla UKC
- **Status:** `[OPEN - BLOCKING]`
- **Kontekst:** `STEP-12` | Moduł: `physics::ukc` | Plik: `src/physics/ukc.rs`
- **Odpowiedź analityka:**
  W MVP pływy są pomijane, a bezpieczna głębokość wynosi 5 m. Jednostka ma
  zanurzenie statyczne 2 m, więc próg 5 m uznano za bezpieczny. Odpowiedź nie
  wskazuje jednak wprost, czy próg odnosi się do głębokości batymetrycznej, czy
  pozostałego UKC. Mechanizm wyznaczania głębokości z użyciem pływów należy
  dodać w przyszłości.
- **Opis problemu / brakującej wiedzy:**
  Prosimy jawnie potwierdzić, czy 5 m oznacza minimalną głębokość z batymetrii,
  czy minimalny pozostały UKC. Wzory squat/przechyłu wymagają też współczynnika
  blokowego `C_b` i szerokości kadłuba. Odpowiedź podaje zanurzenie statyczne
  2 m dla łodzi, ale nie definiuje źródła pozostałych parametrów dla `vessel_id`
  ani czy squat/przechył mają być używane do odrzucania tras w MVP.
- **Zastosowane obejście / tymczasowe założenie (Fallback):**
  Brak – odpowiedź jest zapisana, ale nie rozstrzyga jednoznacznie znaczenia
  progu ani źródła hydrostatyki. Nie zakładamy wartości `C_b` lub szerokości;
  STEP-12 pozostaje zablokowany.