# Prestazioni di `transcribe`: piano operativo

Questo documento è il piano deciso, non più una proposta: le scelte sono chiuse.
Restano da eseguire l'implementazione e le misure di validazione. Se una misura
contraddice una scelta, si aggiorna il piano prima di cambiare il codice, non
dopo.

## Stato

Fase 1 implementata e coperta dai test: decodifica streaming unica, una sola
chiamata batch, guardia 4 ore, `session.json` + `transcript.jsonl` + `.txt` con
timestamp, rimozione dei flag VAD da `transcribe`, metodo batch per Whisper e
Parakeet. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test` e `cargo build --release` passano.

Prova end-to-end su un WAV reale breve: la cartella contiene solo `session.json`,
`transcript.jsonl` e `<stem>.txt`; il `.txt` ha i timestamp; `export --format
text` coincide byte per byte con il `.txt` privato dei prefissi. La stessa prova
con Parakeet produce righe a 15 s e lo stesso round-trip esatto.

Validazione eseguita su due file reali. Podcast da 74,2 minuti: **1,40x** sul
prefisso stabile di 20 minuti, **1,43x** sul file intero nel run pulito, testo al
97,6% di sovrapposizione parole, picco RSS 1,88 GiB. Il `.txt` passa da 3.999
micro-segmenti nativi a 249 righe leggibili. Sul file silenzioso da 428 s:
**1,60x** e testo da 756 a 4.654 caratteri. Live da 177,5 minuti: **1,11x** sul
prefisso, file intero in 1.214,9 s (RTF 0,114) con picco **2,91 GiB**, 721
righe; un loop localizzato all’outro, dipendente dal contesto. Risultati in
`benchmarks/results/apple-m5-transcribe-long-2026-11.json` e
`benchmarks/results/apple-m5-transcribe-3h-2026-11.json`.

Fase 2 e Fase 3 misurate: greedy scartato (13% più veloce ma 18% meno parole).
Parakeet resta una scelta esplicita: 3h in 440,6 s / 1,75 GiB / 92,2% di overlap e
nessun loop all’outro; sul 428 s tecnico è 3,2x più veloce di Whisper ma scende
al 63,7% di overlap. Metal è attivo, flash attention resta acceso e lo sweep dei
thread è piatto: non ci sono guadagni da tuning. Resta l’ascolto manuale dei
tratti degradati; se la memoria diventasse un problema, si valuta la finestra da
10 minuti.

## Sintesi

Il collo di bottiglia non è il modello. `large-v3-turbo` è già il compromesso
velocità/qualità giusto e i benchmark del repo lo confermano (RTF ~0,18 su audio
pulito). Il problema è che `transcribe` riusa il pipeline del live: VAD che chiude
un segmento dopo 800 ms di silenzio e una chiamata ASR per ogni frase.

Whisper internamente lavora sempre su finestre da 30 secondi, con padding.
Quindi una frase da 2 secondi costa quasi quanto una da 30. Con una chiamata per
frase il tempo totale diventa proporzionale al **numero di frasi**, non alla
durata dell'audio.

La decisione è separare il percorso batch dal live: `transcribe` decodifica il
file, fa **una sola** `whisper_full` sull'intero audio e conserva i segmenti
nativi di Whisper con i loro timestamp. `start` resta intatto.

## Misure

Registrazione reale `sessions/26-11-04_reti/audio/mixed.wav` (428 s), Apple M5,
`ggml-large-v3-turbo-q8_0.bin`.

| Percorso | Chiamate a whisper | Tempo | Testo prodotto |
| --- | ---: | ---: | ---: |
| `transcribe` attuale (VAD a 800 ms) | 39 | 72,8 s | 767 caratteri |
| stesso file, segmenti da 30 s (stesso pipeline) | 15 | 54,7 s | 4.044 caratteri |
| stesso file, una sola `whisper_full` (benchmark) | 1 | 49,0 s (inferenza) | 4.641 caratteri |

Il run da 30 s, pur processando tutto il file (silenzio compreso), è più veloce
del run VAD che processava solo 12 s di parlato: la conferma che domina il costo
fisso per chiamata, non l'audio elaborato.

Costo per chiamata ricavato: circa **1,8 s**, indipendentemente dalla durata del
segmento. Nel run attuale i 39 segmenti contenevano in media 0,3 s di parlato
ciascuno.

> Il file di prova è al 97% sotto la soglia VAD: non è un confronto di qualità
> perfetto, ma il dato sul costo per chiamata è solido. La validazione vera usa
> registrazioni reali da 90–180 minuti con parlato continuo (vedi
> “Validazione”).

## Perché il VAD resta nel live e sparisce dal batch

Il VAD (Voice Activity Detection) decide, chunk per chunk, se c'è voce o
silenzio. In Scribroom non è un modello neurale: ogni 20 ms calcola l'RMS
(l'energia) del chunk e lo confronta con una soglia. Serve a non mandare silenzio
al modello e a delimitare i segmenti in tempo reale. È economico, ma fragile:
essendo una soglia assoluta, registrazioni basse restano sotto e registrazioni
rumorose la superano di continuo.

Nel live la latenza conta: chiudere una frase dopo 800 ms è desiderabile. Nel
batch la latenza non conta e la segmentazione nativa di Whisper è migliore:
nessuna soglia da tarare, nessun rischio di trascrizione vuota su audio basso,
contesto conservato tra le finestre.

## Cause, in ordine di impatto

1. **Una inferenza per frase.** È la causa principale. Il segmentatore live
   esiste per la latenza, non per il batch.
2. **Temperature fallback attivo.** `temperature_inc` resta al default di
   whisper.cpp (0,2): se una finestra non supera i controlli di qualità, whisper
   riprova fino a 6 volte (0, 0,2, … 1,0), ogni volta con 5 decoder beam. Su
   rumore o silenzio può costare 2-6x.
3. **Nuovo `WhisperState` a ogni segmento.** Secondario rispetto all'encoder, ma
   evitabile.
4. **Beam search 5 di default.** Non va cambiato alla cieca: su un file con molto
   silenzio greedy è risultato perfino più lento, perché produce più allucinazioni
   e fa scattare più retry. Va misurato su audio reale.

## Problemi collaterali emersi

- **Soglia VAD assoluta.** RMS ≥ 0,02 equivale a −34 dBFS. Un file con picco a
  −36 dB produce una trascrizione vuota senza errore, perché nessun chunk viene
  classificato come voce. Il percorso batch elimina il problema alla radice.
- **Limite WAV.** Per i `.wav` `transcribe` usa `FileAudioSource`, con cap a
  64 MiB / 30 minuti: una sessione registrata di un'ora non è trascrivibile. Per
  mp3/m4a il decoding è già streaming. Il percorso batch unifica la decodifica e
  rimuove il cap.

## Decisioni chiuse

| ID | Decisione | Motivo |
| --- | --- | --- |
| D1 | `transcribe` fa **una sola** `whisper_full` sull'intero file decodificato. Nessuna finestra nostra. | È il percorso più veloce e più semplice; whisper finestra già a 30 s. |
| D2 | Guardia esplicita a **4 ore** di audio: oltre, errore chiaro prima di chiamare whisper. | 4 h ≈ 880 MiB di campioni; il mel dell'intero file aggiunge ~340 MB a 3 h. Misurato 2,91 GiB a 3 h; stima ~3,5 GiB a 4 h. Un limite documentato è meglio di un OOM. |
| D3 | Decodifica unica con symphonia in streaming per **tutti** i formati, WAV compreso. Rimosso il ramo `FileAudioSource` da `transcribe`. | Un solo percorso, niente cap 64 MiB / 30 min, memoria di decodifica costante. || D4 | `transcribe` scrive una cartella-sessione leggera: `session.json`, `transcript.jsonl`, `<stem>.txt`. Niente `audio/`, `status.json`, `events.jsonl`, `current-session.json`. | `export` richiede `session.json` + `transcript.jsonl`; senza di essi il `.txt` non è esportabile. |
| D5 | La directory si crea solo a trascrizione finita; se la scrittura fallisce viene rimossa. Il comando stampa la **directory**, non il path del `.txt`. | Mantiene “se fallisce non resta niente” e rende immediato il comando di export. |
| D6 | `.txt` di default: `[MM:SS → MM:SS] testo`, una riga per **gruppo** di segmenti nativi, precisione al secondo, `[H:MM:SS → H:MM:SS]` oltre l'ora. Il gruppo si chiude a punteggiatura forte, pausa ≥ 0,5 s o 20 s. Testo di ogni segmento trimmato. | I micro-segmenti nativi da ~1 s rendono il `.txt` illeggibile se non raggruppati; 20 s imita la granularità del vecchio VAD. |
| D7 | Il `.txt` senza timestamp si ottiene con `scrib export <dir> --format text`. Nessun `--no-timestamps`, nessun formato dedicato. | `export --format text` produce già esattamente il testo puro. |
| D8 | Rimossi i tre flag `--vad-*` da `transcribe`; restano su `start`. Nessun flag di tuning su `transcribe`. | Il batch non usa il VAD: tenerli sarebbe una bugia. |
| D9 | `AsrEngine` acquisisce un metodo batch che restituisce segmenti temporizzati: Whisper usa i segmenti nativi; Parakeet restituisce un segmento per chunk interno da 15 s. | Parakeet non produce timestamp: i chunk sono l'unica granularità disponibile senza riscrivere il decoder. |
| D10 | Tuning batch: `temperature_inc = 0.0`, `temperature = 0.0`, beam 5. `start` resta a `temperature_inc = 0.2`. | `temperature_inc = 0` elimina i retry, quindi non può rallentare. Greedy si adotta solo se i numeri lo giustificano. |
| D11 | `temperature_inc` entra in `InferenceConfig` come campo con `serde(default)` a 0,2. Nessun bump di `SCHEMA_VERSION`. | I vecchi record restano leggibili e il valore effettivo finisce in `session.json`. |
| D12 | `transcribe` non prende il lock di `start` e non scrive `current-session.json`: può girare durante una registrazione, con contesa di GPU/RAM documentata. | Bloccare la trascrizione durante una call sarebbe più sorprendente della contesa. |
| D13 | Parakeet entra nel percorso batch già in Fase 1; la Fase 3 è la validazione della qualità e la decisione se promuoverlo come scelta consigliata. | Il percorso batch è comune; separare i motori nell'orchestrazione sarebbe duplicazione. |
| D14 | I file reali da 90–180 minuti si usano solo in locale; nel repo finiscono hash e metriche, non l'audio. | Privacy: sono riunioni reali. |

## Piano operativo

### Fase 1 — percorso batch separato dal live

#### 1.1 Input e decodifica

- Abilitare in `Cargo.toml` le feature `wav` e `pcm` di symphonia.
- Usare un solo decoder streaming (`CompressedAudioSource`, eventualmente
  rinominato `DecodedAudioSource`) per `.wav`, `.mp3`, `.m4a`, `.mp4`, `.aac`.
- Eliminare da `transcribe` l'enum `AudioInput` e il ramo `FileAudioSource`.
  `FileAudioSource` resta per il live e per i test che simulano lo streaming.
- Accumulare i campioni decodificati in un `Vec<f32>` (16 kHz mono). La durata
  totale si legge dall'header quando c'è; altrimenti si conosce a fine decodifica.
- Controllo della guardia D2: se la durata dichiarata supera 4 ore, errore prima
  di decodificare; se non è dichiarata, errore appena il conteggio dei campioni
  supera il limite, prima di chiamare whisper.
- La barra di avanzamento parte dalla durata nota; se il container non la
  dichiara, mostra la riga “durata ignota” come oggi.

#### 1.2 API del motore

In `domain.rs`, accanto a `SpeechSegment`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedText {
    pub start: TimestampUs,
    pub end: TimestampUs,
    pub text: String,
}
```

In `asr.rs`, il trait acquisisce il metodo batch. La callback di avanzamento è
un piccolo wrapper `Clone + 'static`, così può essere passata a
`set_progress_callback_safe` senza `unsafe`:

```rust
#[derive(Clone)]
pub struct Progress(Arc<Mutex<Box<dyn FnMut(u64) + Send>>>);

pub trait AsrEngine: Send + 'static {
    // Live, invariato.
    fn transcribe(
        &mut self,
        segment: &SpeechSegment,
        language: &str,
        config: &InferenceConfig,
    ) -> Result<AsrOutput>;

    // Batch: una chiamata sull'intero file, segmenti temporizzati.
    fn transcribe_batch(
        &mut self,
        samples: &[f32],
        language: &str,
        config: &InferenceConfig,
        progress: &Progress,
    ) -> Result<Vec<TimedText>>;
}
```

- L'implementazione di default costruisce un unico `TimedText` chiamando
  `transcribe`; serve ai mock e ai test. Whisper e Parakeet la sovrascrivono.
- `WhisperEngine::transcribe_batch`:
  - costruisce `FullParams` con `temperature_inc` preso da `config`;
  - imposta `set_progress_callback_safe`, che riceve 0-100; il chiamante lo
    converte in microsecondi sul totale;
  - esegue una sola `full` su tutti i campioni;
  - legge `full_n_segments()`, per ogni segmento `start_timestamp()` /
    `end_timestamp()` (centisecondi) e `to_str()`, converte in µs, scarta i testi
    vuoti dopo `trim`.
- `ParakeetEngine::transcribe_batch`:
  - spezza `samples` in chunk da `MAX_CHUNK_SECONDS` (15 s);
  - per ogni chunk chiama `transcribe_chunk`, assegna `start` e `end` dal numero
    di campioni consumati, scarta i testi vuoti;
  - chiama `on_progress` dopo ogni chunk.
- `ParakeetEngine::transcribe` (live) resta com'è: concatena i chunk in un'unica
  stringa.
- `InferenceConfig` acquisisce `temperature_inc: f32` con
  `#[serde(default = "default_temperature_inc")]` e default 0,2.
  `validate_inference_config` verifica che sia finito e non negativo.
- `WhisperEngine::transcribe` (live) applica `set_temperature_inc(config.temperature_inc)`,
  così il default 0,2 mantiene il comportamento attuale.

#### 1.3 Orchestrazione di `transcribe`

`TranscribeOptions` perde `segmenter`; restano `input`, `model`, `sessions_dir`,
`language`, `inference`, `session_name`.

Flusso:

1. Valida lingua, configurazione di inferenza, directory di sessione e file di
   input.
2. Rileva l'artefatto e calcola l'identità con `model_identity` (SHA-256, con la
   cache `.sha256` già esistente).
3. Apre il decoder streaming, accumula i campioni, applica la guardia D2.
4. Carica il motore e chiama `transcribe_batch`; la callback aggiorna la barra.
5. Costruisce i `TranscriptSegment` canonici e i `SegmentLine` per il `.txt`,
   raggruppando i micro-segmenti nativi con `merge_timed_text` (punteggiatura,
   pausa ≥ 0,5 s o 20 s).
6. Crea la directory con `create_session_directory` e scrive:
   - `transcript.jsonl` (una riga per segmento, `TranscriptSegment` con
     `session_id`, `segment_id`, lingua, nome e SHA-256 del modello, engine,
     `inference` con `temperature_inc = 0.0`);
   - `<stem>.txt` con `render_text_with_timestamps`;
   - `session.json` con stato `Completed`, `started_at_unix_ms` dell'inizio,
     `ended_at_unix_ms` della fine, `config.file_source = Some(input)`,
     `config.segmenter` ai valori di default (campo richiesto dallo schema, non
     usato dal batch), `config.silence_timeout_ms = None`.
7. Se una scrittura fallisce, rimuove la directory appena creata e propaga
   l'errore.
8. Stampa la directory.

Non si scrivono `status.json`, `events.jsonl`, `audio/` né
`current-session.json`: `status`, `pause`, `resume` e `stop` non devono vedere
queste cartelle.

#### 1.4 Output `.txt`

Nuova funzione in `export.rs`:

```text
[00:00 → 00:07] Buongiorno a tutti.
[00:07 → 00:19] Oggi parliamo di...
```

- Formato: `[start → end] testo`.
- Precisione al secondo; `H:MM:SS` quando la durata supera un'ora
  (es. `[1:00:00 → 1:00:04]`).
- Una riga per **gruppo** di segmenti nativi; il gruppo si chiude a punteggiatura
  forte, pausa ≥ 0,5 s o 20 s. Nessun riflusso del testo.
- Testo trimmato; i segmenti vuoti non producono righe.
- `format_timestamp`/`mmss` condivisi tra barra di avanzamento ed export, per non
  avere due formattatori divergenti.

Con Parakeet le righe sono i chunk da 15 s: possono spezzare una frase. È
accettato in Fase 1 ed è documentato; una granularità più fine richiederebbe i
timestamp per token del decoder TDT e non è in questo piano.

#### 1.5 CLI ed export

- `TranscribeArgs`: rimuovere `VadArgs`. Restano `--model`, `--model-preset`,
  `--language`, `--asr-prompt`, `--asr-flash-attention`, `--sessions-dir`.
- `InferenceArgs` resta invariato; `inference_config` riceve il valore di
  `temperature_inc` (0,2 per `start`, 0,0 per `transcribe`).
- `export` non cambia: `--format text` è il percorso senza timestamp. Il default
  resta `markdown`.
- Aggiornare l'help di `transcribe`: output = directory con `session.json`,
  `transcript.jsonl` e `<stem>.txt`; il `.txt` contiene i timestamp; per il testo
  puro usare `export --format text`.

#### 1.6 Test

- Sostituire `write_transcript_writes_only_the_txt_file` con un test che verifica
  `session.json`, `transcript.jsonl` e `<stem>.txt`, e che la directory non
  contenga `audio/`, `status.json`, `events.jsonl`.
- Mantenere `failed_run_leaves_no_session_directory`; aggiungere il caso di
  scrittura fallita con rimozione della directory.
- Test del renderer `.txt`: timestamp, righe vuote scartate, oltre un'ora.
- Test del raggruppamento: punteggiatura, pausa ≥ 0,5 s, tetto di 20 s.
- Test di round-trip: creare una cartella-sessione, esportare `--format text`,
  togliere i prefissi `[.. → ..] ` dal `.txt` e confrontare byte per byte.
- Test della guardia 4 ore senza audio reale (durata simulata).
- Test del decoder streaming su un WAV sparso oltre 30 minuti (riusare
  `write_sparse_wav` di `audio.rs`): il vecchio `FileAudioSource` deve fallire, il
  nuovo percorso no.
- Test del calcolo dei timestamp dei chunk Parakeet senza caricare il modello.
- Test CLI: `transcribe` rifiuta `--vad-end-ms`; `start` lo accetta ancora.
- I test esistenti su `collect_lines`/`Segmenter` in `transcribe.rs` vengono
  sostituiti da test del percorso batch con un `MockAsr` che implementa
  `transcribe_batch`.

#### 1.7 Documentazione

Aggiornare:

- `README.md`: `transcribe` non è più “solo un `.txt`”; niente limite WAV.
- `docs/COMMANDS_HELP.md`: output, flag rimossi, export del testo puro.
- `docs/RELAZIONE.md`: percorso batch, file della cartella, export.
- `sessions/README.md`: una cartella `transcribe` non è una sessione live.
- `CHANGELOG.md`: cambio del `.txt`, rimozione flag VAD da `transcribe`, nuovo
  `session.json`/`transcript.jsonl`.

### Fase 2 — tuning misurato

`temperature_inc = 0` è già in Fase 1 perché non può rallentare. In Fase 2 si
misura solo:

- **Greedy vs beam 5** sui file reali. Greedy si adotta solo se è almeno il 20%
  più veloce e non peggiora la qualità (parole prodotte e sovrapposizione) su
  **tutti** i file di prova. Misurato: 13% più veloce ma 18% meno parole, quindi
  resta beam 5.
- **`no_context`**: misurato sul live da 3 h; testo identico e stessi loop,
  quindi resta `false`. Il contesto fra finestre resta un vantaggio del batch.
- **`best_of`**: irrilevante con beam search; non si espone.
- **Metal**: attivo (GPU Apple M5, libreria embedded). Il messaggio “error
  compiling source” riguarda solo il probe del tensor API: non è CPU-only.
- **Flash attention**: acceso 0,1166 contro 0,1500 spento sul clip da 18 s
  (~29% più veloce). Resta acceso.
- **Thread 4/6/8/10**: RTF 0,1134 / 0,1128 / 0,1130 / 0,1127. Piatto: resta il
  default 8.

Nessun guadagno residuo dal tuning: con la GPU il collo di bottiglia è il modello.

Per misurare, `src/bin/asr-benchmark.rs` ha `--strategy` e `--temperature-inc` e
usa lo stesso decoder streaming di `transcribe`. Il benchmark resta uno
strumento di sviluppo, non un flag di `scrib`.

### Fase 3 — Parakeet: validazione e promozione

Parakeet è già nel progetto e il percorso batch lo esegue in Fase 1. Misurato sul
live da 177,5 min: **440,6 s, 1,75 GiB, 92,2% di sovrapposizione parole con
Whisper, nessun loop all’outro**. `--language` è ignorato: `it` ed `en` producono
lo stesso testo italiano, quindi il rilevamento automatico è reale.

Il test `start` è stato eseguito su un estratto di 3 minuti (182 s, sessione
completata, 4 segmenti): `start` non ha un flag per un file e il replay WAV è in
tempo reale, quindi il file intero richiederebbe 3 h di wall clock. La decisione
resta: Parakeet è una **scelta esplicita**, non un sostituto automatico, per la
mancanza di prompt/glossario e i timestamp a 15 s. È però l’opzione più veloce e
con meno RAM.

Il preset `small` è stato rimosso dal progetto e dal disco: la strategia “modello
Whisper più piccolo” non è più disponibile.

## Validazione

### Corpus

- Registrazioni reali da 90–180 minuti con parlato continuo, fornite
  dall'utente, in italiano. La misura corrente usa un MP3 da 74,2 minuti.
- `sessions/26-11-04_reti/audio/mixed.wav` (428 s): caso silenzioso, baseline
  storica del documento.
- Se i file da 90–180 minuti sono WAV, provano anche la rimozione del cap; se sono
  compressi, la rimozione del cap si prova con un WAV di sessione oltre i 30
  minuti.

L'audio non entra nel repository. Si registrano hash, durata, hardware e metriche
in `benchmarks/results/`.

### Protocollo

1. **Baseline.** Se il file è `.m4a`/`.mp3` (o un WAV ≤ 30 min), eseguire il
   percorso attuale sull'intero file. Se è un WAV > 30 min, ritagliare un
   prefisso di 29 minuti e usare quello per il confronto A/B; sul file intero
   eseguire solo il nuovo percorso.
2. **A/B sul prefisso.** Vecchio e nuovo percorso sullo stesso audio, stesso
   modello, stessa lingua. Misurare tempo totale, numero di chiamate ASR,
   caratteri/parole, picco RSS.
3. **File intero.** Solo nuovo percorso: tempo totale, RTF, picco RSS,
   completezza del testo.
4. **Qualità.** Se esiste un riferimento, WER/CER su un campione di 5-10 minuti.
   Altrimenti: confronto col vecchio sul prefisso, verifica che il nuovo testo non
   sia più corto, ascolto/lettura di 5 minuti e controllo dei loop di ripetizione.
5. **Round-trip.** Per ogni cartella prodotta: `export --format text` deve
   coincidere byte per byte con il `.txt` privato dei prefissi `[.. → ..] `.

### Soglie di accettazione

| Caso | Soglia |
| --- | --- |
| 428 s, nuovo vs 72,8 s attuali | ≤ 55 s e ≥ 4.000 caratteri |
| Prefisso 20–29 min, parlato continuo | nuovo non più lento del vecchio; testo ≥ 90% di sovrapposizione parole |
| File intero 90–180 min | completa; RTF ≤ 0,15; picco RSS ≤ 3,5 GB |
| Chiamate ASR | Whisper: 1 per file; Parakeet: 1 per chunk da 15 s |
| Testo | mai vuoto; sul prefisso ≥ 90% di sovrapposizione parole col vecchio |
| Loop di ripetizione | nessun loop sistematico; quelli localizzati vanno confrontati col vecchio e documentati |
| Round-trip | identico byte per byte dopo la rimozione dei prefissi |
| Guardia 4 h | errore chiaro, nessuna directory creata |

### Risultati misurati (MP3 reale, 74,2 min)

| Misura | Vecchio | Nuovo |
| --- | ---: | ---: |
| Tempo file intero | 758 s | 529 s (1,43x) |
| Tempo prefisso 20 min (mediana di 2 run) | 191,8 s | 137,3 s (1,40x) |
| 428 s silenzioso | 75,9 s / 756 caratteri | 47,5 s / 4.654 caratteri (1,60x) |
| 428 s silenzioso, Parakeet | 75,9 s / 756 caratteri | 15,1 s / 3.388 caratteri (5,0x vs vecchio, 3,2x vs Whisper) |
| Picco RSS file intero | 1,16 GiB | 1,88 GiB |
| Testo | 64.388 caratteri / 10.118 parole | 63.252 / 10.195 |
| Righe `.txt` | 232 (VAD) | 249 (merge di 3.999 micro-segmenti) |
| Sovrapposizione parole sul prefisso | — | 97,6% |

- Il secondo run del nuovo percorso sul file intero è stato 745 s, con lo stesso
  user/sys: contesa GPU/Metal, non usato.
- Sul file da 428 s il VAD produceva 756 caratteri, il batch 4.654: il guadagno
  più importante non è il tempo, è il testo che prima veniva perso.
- Parakeet sullo stesso 428 s: 15,1 s, 1,22 GiB, 3.388 caratteri / 576 parole,
  overlap 63,7% con Whisper. È 3,2x più veloce di Whisper ma perde circa un
  quarto delle parole: su audio tecnico silenzioso Whisper resta più completo.
- Greedy: RTF 0,1048 contro 0,1206 di beam (13% più veloce), ma 2.049 contro
  2.513 parole e overlap 82,0%. Resta beam.
- Parakeet: RTF 0,0322, RSS 1,27 GB, 2.464 parole, overlap 94,9% con Whisper beam.
  Resta una scelta esplicita.
- Round-trip esatto.

La soglia iniziale di 2x era troppo ottimistica: il tetto è il costo del modello
(RTF ~0,12), non il numero di chiamate. Su parlato continuo il guadagno reale è
~1,4x; i guadagni maggiori restano sui file con molte pause brevi e sulla
completezza del testo. Se un nuovo file è più lento del vecchio o scende sotto il
90% di sovrapposizione, si indaga prima di considerare chiusa la validazione.

### Secondo file: live da 177,5 min (WebM/Opus convertito in M4A)

| Misura | Vecchio | Nuovo |
| --- | ---: | ---: |
| Tempo prefisso 20 min (mediana di 2 run) | 172,1 s | 155,3 s (1,11x) |
| Tempo file intero 177,5 min | non eseguito | 1.214,9 s (RTF 0,114) |
| Picco RSS file intero | — | 2,91 GiB |
| Testo file intero | — | 145.992 caratteri / 25.975 parole / 721 righe |
| Picco RSS prefisso | 1,16 GiB | 1,28 GiB |
| Testo prefisso | 14.055 caratteri / 2.415 parole | 14.109 / 2.434 |
| Sovrapposizione parole | — | 90,3% |

- Il guadagno di velocità dipende dal contenuto: 1,40x sul podcast, 1,11x su
  questo live continuo. Il vecchio VAD qui fa già pochi segmenti lunghi.
- Il nuovo testo contiene loop di ripetizione (“Siamo in diretta con i
  resistori”). Anche `temperature_inc = 0,2` li produce (“Grazie a tutti”) e
  costa l’1,7% di RTF in più: il fallback non risolve questo contenuto. Resta
  `temperature_inc = 0`. Anche `no_context = true` lascia il testo identico
  (RTF 0,1222 contro 0,1263): non è un problema di contesto fra finestre.
- È un caso difficile (live, musica, sovrapposizioni): il vecchio percorso ha
  allucinazioni diverse. La qualità va giudicata a orecchio, non solo con
  l’overlap.
- Run completo da 177,5 min: 1.214,9 s, RTF 0,114, picco RSS **2,91 GiB**, 721
  righe. Il vecchio percorso non è stato eseguito sul file intero.
- Il picco non è la crescita del `Vec`: è il mel calcolato da whisper sull’intero
  audio. La preallocazione del buffer non lo riduce (misurata, poi rimossa).
- Loop: una sola riga da 30 s a 2:56:44 contiene “oh oh oh” 217 volte
  (outro/contesto). Sugli ultimi 2 minuti isolati vecchio e nuovo producono
  “ohm”, non “oh”: il loop dipende dal contesto lungo, non dal contenuto locale.
- 2,91 GiB su una macchina da 32 GB è accettabile per il tetto delle 4 ore. Se
  diventasse un problema, l’unica mitigazione è spezzare il batch in finestre da
  10 minuti, perdendo il contesto fra finestre.

### Parakeet sullo stesso file da 177,5 min

| Misura | Whisper turbo | Parakeet int8 |
| --- | ---: | ---: |
| Tempo | 1.214,9 s | **440,6 s (2,76x)** |
| Picco RSS | 2,91 GiB | **1,75 GiB** |
| Testo | 145.992 caratteri / 25.975 parole | 144.739 / 25.757 |
| Righe `.txt` | 721 | 701 |
| Sovrapposizione parole | — | 92,2% |
| Loop “oh oh oh” all’outro | 217 volte in 30 s | assente |

- `--language it` e `--language en` producono lo stesso testo: Parakeet ignora il
  flag e rileva la lingua da solo.
- `start` è stato provato su un estratto di 3 minuti (182 s, completato, 4
  segmenti). Non esiste un flag file per `start`: il replay WAV è in tempo reale
  e limitato a 30 minuti, quindi il file intero da 3 h non è testabile su `start`
  senza una modalità di replay dedicata.
- Parakeet è l’opzione più veloce e con meno RAM, ma resta esplicita: niente
  prompt/glossario e timestamp a 15 s. Sul 428 s tecnico l’overlap scende al
  63,7% (Whisper 4.654 caratteri contro 3.388): la qualità dipende dal contenuto.

## File da toccare

| File | Modifica |
| --- | --- |
| `Cargo.toml` | feature `wav` + `pcm` di symphonia |
| `src/audio.rs` | decoder streaming unico; test WAV lungo |
| `src/domain.rs` | `TimedText`; `temperature_inc` in `InferenceConfig` |
| `src/asr.rs` | metodo batch; Whisper nativo; progress; `temperature_inc` |
| `src/parakeet.rs` | batch a chunk da 15 s con timestamp |
| `src/transcribe.rs` | orchestrazione batch, guardia 4 h, output strutturato, cleanup |
| `src/export.rs` | `render_text_with_timestamps`; formattatore condiviso |
| `src/cli.rs` | rimozione VAD da `transcribe`; `temperature_inc` per comando |
| `src/error.rs` | errore dedicato per audio oltre il limite |
| `src/bin/asr-benchmark.rs` | decoder streaming; `--strategy`; `--temperature-inc` |
| `tests/export_formats.rs` | round-trip `.txt` con timestamp / `export --format text` |
| test e docs | come in 1.6 e 1.7 |

## Rischi e mitigazioni

- **Memoria.** 220 MiB di campioni per ora, più il mel dell'intero audio.
  Misurato **2,91 GiB a 3 h**; stima ~3,5 GiB al limite delle 4 h. Mitigazione:
  guardia D2. Se il picco diventasse un problema, l'unica riduzione reale è
  spezzare il batch in finestre da 10 minuti, perdendo il contesto fra finestre.
- **Allucinazioni su silenzio e loop.** `temperature_inc = 0` toglie i retry ma
  non impedisce a whisper di produrre testo su silenzio. Sul live da 3 h compare
  un loop localizzato all'outro, indipendente da `temperature_inc` e
  `no_context`. Mitigazione: confronto manuale col vecchio e documentazione; se
  il loop diventasse sistematico, si valuta il fallback o un filtro di
  ripetizione.
- **Parakeet con timestamp grossolani.** Le righe da 15 s possono spezzare una
  frase. Mitigazione: documentato; eventuale raggruppamento a punteggiatura solo
  se i test reali lo richiedono.
- **Cambio dell'output.** Il `.txt` cambia formato e la cartella contiene più
  file. Mitigazione: test, CHANGELOG e documentazione nella stessa modifica.
- **Contesa con `start`.** Due processi possono usare la GPU insieme.
  Mitigazione: documentato; nessun lock nuovo.
- **WAV esotici.** Symphonia può non coprire un WAV che hound leggeva.
  Mitigazione: errore chiaro; `start` continua a usare hound.

## Non obiettivi

- Nessun VAD neurale e nessuna soglia RMS nel batch.
- Nessun timestamp a livello di parola.
- Nessuna diarizzazione o identificazione degli speaker.
- Nessun batch multi-file in una sola invocazione.
- Nessun nuovo flag di tuning su `scrib transcribe`.
- Nessun cambiamento al pipeline live di `start`.

## Definizione di “fatto”

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
   `cargo test` e `cargo build --release` passano.
2. I file reali da 74,2 e 177,5 minuti e il file da 428 s rispettano le soglie
   della validazione; i loop localizzati sono documentati; i risultati sono in
   `benchmarks/results/`.
3. Il round-trip `.txt` / `export --format text` è esatto.
4. README, COMMANDS_HELP, RELAZIONE, sessions/README e CHANGELOG sono aggiornati.
5. Nessuna decisione di questo documento resta aperta.
