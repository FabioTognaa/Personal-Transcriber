# Analisi prestazioni di `transcribe`

Documento di **analisi**: nessuna modifica al codice. Serve a decidere *cosa*
implementare, con quali leve e in quale ordine.

- **Obiettivo:** ridurre la durata di `transcribe` e il calore prodotto.
- **Perimetro:** `transcribe` (file → `.txt`). Il percorso live (`start`) è fuori
  perimetro: non presenta problemi di calore.
- **Criterio "gratis":** nessuna perdita di accuratezza misurabile (output a
  confronto prima/dopo). Gli interventi con costo in qualità sono elencati a parte.
- **Piattaforma di riferimento:** MacBook Air M5 (`Mac17,3`) — 10 core (4
  performance + 6 efficienza), 32 GiB, **senza ventola**.
- **Branch:** `dev/perf-transcribe`.

## 1. Contesto hardware: perché il calore è il problema centrale

Il MacBook Air M5 è **fanless**. Sotto carico sostenuto (20+ minuti di Metal a
piena accelerazione) il chip raggiunge la soglia termica e **rallenta da solo**
(thermal throttling). Su una macchina così, il calore non è un effetto collaterale
del tempo: **è una delle cause del tempo**. Conseguenza pratica:

> ridurre il lavoro per minuto di audio rende il programma **più veloce e più
> fresco allo stesso tempo**. Non è un compromesso: è la stessa leva.

Questo orienta tutte le raccomandazioni "gratis" verso *meno lavoro*, non verso
*più accelerazione*.

## 2. Dove va il tempo

Pipeline attuale di `transcribe` (`src/transcribe.rs`):

1. decodifica del file (streaming, `src/audio.rs`) → PCM mono 16 kHz;
2. segmentazione VAD a energia RMS (`src/segment.rs`) in segmenti di parlato;
3. **una chiamata Whisper per ogni segmento**, in sequenza, sullo stesso modello
   (`append_segments` → `WhisperEngine::transcribe`).

Costi dominanti, in ordine:

| Costo | Dove | Nota |
| --- | --- | --- |
| Encoder su finestra fissa da 30 s | per **ogni** segmento | Whisper elabora sempre 30 s: un segmento da 2 s costa quasi come uno da 30 s |
| Decoder in **beam search** (`beam_size = 5`) | per ogni segmento | ~3–5× il costo di greedy, a parità di modello |
| Ricostruzione dello **stato** Whisper | per ogni segmento | `WhisperEngine::transcribe` chiama `context.create_state()` a ogni segmento, non lo riusa |
| Thread = **8** su 10 core (4P + 6E) | per tutto il run | include gli E-core: sovraccarico + calore, spesso senza guadagno di velocità |
| Caricamento modello | una volta | 0,66 s per `large-v3-turbo-q8_0`: trascurabile su file lunghi |
| Decodifica audio | streaming | marginale rispetto all'inferenza |

### Baseline misurata

Numeri già presenti nel repo (`benchmarks/results/`) più la misura reale dichiarata:

| Misura | Durata audio | RTF | Note |
| --- | ---: | ---: | --- |
| fixture `italian_synthetic.wav`, `small-q5_1` | 12,46 s | 0,072 | fixture molto corta, pulita |
| fixture `italian_synthetic.wav`, `large-v3-turbo-q8_0` | 12,46 s | 0,182 | include ~0,053 di caricamento modello |
| **reale: video ~90 min** | ~90 min | **≳ 0,22** | "20+ minuti"; il dato è arrotondato |

**Osservazione:** togliendo il caricamento modello, la fixture turbo scende a RTF
≈ 0,13. Il dato reale (~0,22 su 90 min) è quindi **~1,7× più lento della fixture a
parità di modello**. Ipotesi principali, da confermare con una misura controllata:

- **throttling termico** accumulato in 20 minuti su chassis fanless;
- audio reale più difficile (rumore, turni, più segmenti corti → più encoder pass);
- overhead di decodifica del contenitore video.

## 3. Leve "gratis" (nessuna perdita di accuratezza)

Ordinate dal più economico al più invasivo. Ognuna va applicata **da sola** e
verificata (tempo + confronto output) prima della successiva.

### A. Thread: 8 → solo i P-core (4)

- **Situazione:** `default_threads()` in `src/domain.rs` restituisce
  `min(available_parallelism, 8)`; su questa macchina = **8**, includendo gli
  E-core.
- **Perché aiuta:** in whisper.cpp `n_threads` va allineato ai core fisici
  *performance*. Gli E-core non accelerano le matrici e aggiungono calore;
  overscrubscription peggiora latenza e temperatura.
- **Dove:** `src/domain.rs` (`default_threads`), eventualmente ri-esponendo
  `--asr-threads` (rimosso in 1.0.0).
- **Costo:** configurazione. **Effetto atteso:** neutro o più veloce, e **più
  fresco** — la leva più economica, va testata per prima.

### B. Encoder su Core ML / Neural Engine — *come, in dettaglio*

È la leva con il miglior rapporto calore/velocità: l'encoder domina il costo di
`large-v3-turbo` (32 layer di encoder contro 4 di decoder) e il Neural Engine fa lo
stesso lavoro a **energia molto inferiore** rispetto a Metal/CPU. Non richiede
modifiche al codice di inferenza: è **build + un artefatto**.

**1. Build.** La feature esiste già in `Cargo.toml`:

```toml
coreml = ["whisper-rs/coreml"]   # default = ["metal"]
```

Si compila con:

```sh
cargo build --release --features coreml   # metal resta attivo (è nel default)
```

Il build script di `whisper-rs-sys` definisce `WHISPER_COREML=ON` e
`WHISPER_COREML_ALLOW_FALLBACK=1`.

**2. Artefatto encoder.** A runtime whisper.cpp **deriva il percorso dell'encoder dal
percorso del modello**, con una trasformazione di stringa
(`whisper_get_coreml_path_encoder`):

```
models/ggml-large-v3-turbo-q8_0.bin
  → togli estensione   → models/ggml-large-v3-turbo-q8_0
  → togli suffisso -qX_X → models/ggml-large-v3-turbo
  → appendi             → models/ggml-large-v3-turbo-encoder.mlmodelc
```

Quindi va scaricato da `ggerganov/whisper.cpp` (Hugging Face) il file
`ggml-large-v3-turbo-encoder.mlmodelc.zip` e scompattato **accanto al modello**,
con quel nome esatto. Se manca, whisper.cpp logga un errore e — grazie a
`ALLOW_FALLBACK` — **continua su Metal/CPU**: nessun crash, ma nessun guadagno.

**3. Dipendenza critica dallo stato (importante).** In whisper.cpp il contesto Core
ML è **per-*stato*** (`whisper_state.ctx_coreml`, inizializzato in
`whisper_init_state`), non per-context. Il codice attuale crea uno **stato nuovo a
ogni segmento** (`context.create_state()` dentro `WhisperEngine::transcribe`).
Con Core ML attivo questo significherebbe **ricaricare il modello ANE a ogni
segmento** ("first run on a device may take a while…"), probabilmente *peggiorando*
le prestazioni.

> **B dipende da D:** prima si riusa un unico `WhisperState` per tutti i segmenti,
> poi si abilita Core ML. Invertire l'ordine non ha senso.

**4. Rischi.** Il `CHANGELOG` di `whisper-rs` 0.16 annota: *"No MacOS testers were
able to test CoreML support, so it may be broken"*. Va verificato sul campo; in
caso di esito negativo, B si scarta senza perdite (fallback già previsto).

- **Costo:** un download (gratuito) + una build. **Effetto atteso (se funziona):**
  più veloce **e** molto più fresco.

### C. Raggruppare i segmenti in finestre da ~30 s

- **Situazione:** ogni segmento VAD è una chiamata separata, **ciascuna pagata su
  una finestra da 30 s** anche se il segmento dura 3 s.
- **Idea:** concatenare il parlato di segmenti consecutivi (rimuovendo il silenzio
  in mezzo) in finestre fino a ~30 s e fare **una sola chiamata per finestra**.
- **Perché aiuta:** molti meno encoder pass → meno tempo e meno calore. Effetto
  grande su audio conversazionale (frasi corte), modesto su voce lunga continua.
- **Dove:** `src/transcribe.rs` (`collect_lines`/`append_segments`), con una
  funzione che raggruppa i segmenti prima di chiamare l'engine. Il `Segmenter`
  resta invariato (serve al live).
- **Rischio:** i confini dei segmenti cambiano → il testo può cambiare leggermente.
  Il `.txt` di `transcribe` **non contiene timestamp** (`export::render_text` scrive
  solo il testo, una riga per segmento), quindi il formato resta identico: cambia
  solo l'impaginazione delle righe. Da confrontare sull'output reale.
- **Costo:** codice. **Effetto atteso:** da medio a grande, gratis se il testo resta
  equivalente.

### D. Riusare lo `WhisperState` (+ micro)

- **Situazione:** `WhisperEngine::transcribe` crea e distrugge uno stato Whisper a
  ogni segmento. Inizializza cache KV e altro: lavoro ripetuto inutile.
- **Idea:** creare lo stato **una volta** nel motore e riusarlo per tutti i
  segmenti. È anche il prerequisito di B.
- **Micro:** rimuovere il `clone()` della `InferenceConfig` a ogni segmento,
  `FullParams` ricostruito a ogni chiamata, ecc. Guadagno piccolo, ma gratis.
- **Costo:** codice. **Effetto atteso:** piccolo da solo, **abilitante per B**.

### D-bis (opzionale, da verificare). Greedy invece di beam search

- `beam_size = 5` → **1** (`InferenceStrategy::Greedy`, `best_of = 1`).
- Su `large-v3-turbo` il decoder ha 4 layer ed è tarato su greedy: il beam largo
  costa molto e rende poco. Atteso **~1,3–2×** sul decoder.
- **Rientra in "gratis" solo se l'output resta equivalente** sul file reale
  (confronto diretto). In caso contrario passa tra le leve "non gratis".

## 4. Leve non gratis (da valutare, non parte del piano gratis)

Da considerare solo se le leve gratis non bastano. Ognuna **cambia il modello o
l'engine**, quindi va misurata anche in accuratezza.

| Leva | Guadagno atteso | Costo |
| --- | --- | --- |
| Modello più piccolo (`small`/`medium`) | 2–3× | calo di accuratezza (in particolare `small` sbaglia punteggiatura e parole) |
| **Distil-Whisper large-v3.5** | ~1,5× vs turbo | **solo inglese**: inutile per l'italiano |
| **Parakeet TDT 0.6B v3** (Core ML/MLX) | fino a 60–110× realtime | nuovo runtime non-Whisper, sforzo maggiore; IT ~pari, EN migliore |
| **MLX Whisper** (`mlx-whisper`) | ~1,2–2× vs whisper.cpp Metal | dipendenza Python, si perde il binario unico |
| **VAD Silero** (integrato in whisper.cpp) | taglia meglio il silenzio | cambia la segmentazione e quindi il testo |

Riferimento di mercato (ricerca web, da confermare sul proprio audio): su Apple
Silicon il **frontier locale** di velocità è ~10–18× realtime per `large-v3-turbo`
(Core ML/MLX) e ~60–110× per Parakeet; l'accuratezza italiana di `large-v3-turbo`
(~6,4% WER) è dietro Parakeet v3 (~5,6%) e Canary 1B v2 (~5,7%). L'approccio
attuale è **competitivo sul piano privacy**, non su velocità/accuratezza assolute.

## 5. Verifica dell'accuratezza (criterio "gratis")

1. Trascrivere **un file reale** (~10–15 min, italiano) con il binario attuale →
   salvare l'output.
2. Applicare una leva.
3. Ritrascrivere lo stesso file → confrontare i due `.txt` riga per riga.
4. La leva è "gratis" solo se le differenze sono trascurabili (punteggiatura/righe,
   non contenuto).

Nota: la fixture sintetica del repo (12 s, molto pulita) **non basta** per
giudicare la leva C; serve audio reale.

## 6. Rischi e incognite

- **Throttling non ancora misurato:** l'ipotesi che il rallentamento reale derivi
  dal calore va confermata con una misura controllata (tempo + temperatura) prima
  di attribuire i guadagni alla leva giusta.
- **Core ML su `whisper-rs` 0.16 non testato su macOS** (dichiarato dal crate):
  può non funzionare; fallback previsto, scarto senza danni.
- **Leva C cambia la segmentazione:** il contenuto può variare; va confrontato.
- **Nessun riferimento WER** per l'audio reale: la verifica resta un confronto
  testuale, non una metrica.
- **Misure "Mac speed" di mercato** provengono da blog/community, non da benchmark
  indipendenti: da riconfermare sulla propria macchina.

## 7. Ordine di lavoro proposto

1. **Baseline controllata:** trascrivere un file reale *tenendo traccia di tempo e
   calore* (misura iniziale, un solo run, senza modifiche).
2. **A** (thread → P-core): la più economica, misura subito.
3. **D** (riuso dello stato): abilitante e gratis.
4. **B** (Core ML): solo dopo D; verificare che l'encoder venga davvero caricato.
5. **C** (raggruppamento segmenti): gratuita se il testo regge.
6. **D-bis** (greedy): solo se l'output resta equivalente.
7. Le leve "non gratis" (sezione 4) solo se il target non è raggiunto.

Ogni passo: rimisurare tempo/calore e confrontare l'output. Il test di calore finale
è a carico dell'utente, sulla branch dedicata.
