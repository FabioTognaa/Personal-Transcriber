# Modifiche alla qualità di trascrizione

Riepilogo delle modifiche presenti nel working tree (non ancora committate).
Riferimento di progetto: `docs/PIANO_QUALITA.md`.

## 1. VAD adattivo con isteresi — `src/segment.rs`, `src/domain.rs`, `src/cli.rs`

- Prima: `voce = RMS >= 0.02`, soglia fissa, uguale in ogni ambiente.
- Ora: il segmentatore stima il **rumore di fondo** con una media esponenziale
  (`noise_floor`, costante di tempo 1 s) aggiornata sui chunk non-voce.
  - Soglia di **apertura** = `max(rumore × noise_multiplier, energy_threshold)`,
    con tetto di sicurezza 0.25.
  - Soglia di **chiusura** più bassa (metà di quella di apertura, mai sotto il
    rumore): evita il "flicker" al confine voce/silenzio.
- `energy_threshold` (0.02) non è più la regola ma il **minimo manuale**.
- Nuovo campo `noise_multiplier` (default 4.0 ≈ +12 dB), validato.
- Effetto: i confini dei segmenti non cambiano più con il livello del microfono.

## 2. Split sul silenzio invece del taglio rigido — `src/segment.rs`

- Prima: al tetto `max_segment_ms` (30 s) il taglio era rigido, anche a metà parola.
- Ora: si registrano le **pause interne** del segmento attivo (run di silenzio
  ≥ 80 ms). Al tetto si taglia sulla pausa più lunga, con almeno `min_speech_ms`
  di parlato da entrambe le parti.
- Nessun campione perso (`end == start` tra segmenti adiacenti); il resto diventa
  il nuovo segmento attivo.
- Fallback: se non c'è una pausa utile, si taglia al tetto come prima.

## 3. Silenzio di chiusura 800 → 1600 ms — `src/domain.rs`, `src/cli.rs`

- Una pausa breve non spezza più la frase in segmenti da 1–2 parole.

## 4. Contesto tra segmenti (opt-in) — `src/asr.rs`, `src/transcribe.rs`, `src/domain.rs`

- `--asr-context` (default **spento**): porta la coda (~120 caratteri) del testo
  del segmento precedente nel prompt iniziale di Whisper.
- **Cancello**: se il segmento precedente ha `no_speech_probability > 0.6`, la
  coda non viene usata, così un errore precoce non si ripete.
- Si registra il flag `context_enabled` + ampiezza finestra; il prompt dinamico
  **non** viene salvato (ricostruibile dal transcript).

## 5. Confidenza per segmento — `src/asr.rs`, `src/domain.rs`, `src/export.rs`

- `no_speech_probability` (il valore peggiore tra i sotto-segmenti whisper) è
  aggiunta ad `AsrOutput` e a ogni record di `transcript.jsonl`
  (`#[serde(default)]` per i file vecchi).
- Nell'export **Markdown** appare come `(no-speech 0.XX)`; il `.txt` resta pulito.

## 6. Igiene del decoder — `src/asr.rs`

- `set_suppress_nst(true)`: elimina i tag non-parlato tipo `(musica)`.
- Le altre soglie whisper restano ai default.

## 7. Schema v4 e retrocompatibilità export — `src/schema.rs`, `src/export.rs`

- `SCHEMA_VERSION` 3 → 4.
- L'export ora rifiuta solo le versioni **più nuove** (prima qualsiasi versione
  diversa), quindi le sessioni v3 restano esportabili; i campi nuovi hanno
  `#[serde(default)]`.

## 8. Modello di default — `src/cli.rs`

- Default `large-v3-turbo` per `start`, `transcribe` e `doctor` (prima `small`),
  coerente tra i tre comandi.

## 9. Flag VAD completi — `src/cli.rs`

- Esposti da CLI tutti i parametri del segmentatore: `--vad-pre-roll-ms`,
  `--vad-post-roll-ms`, `--vad-min-speech-ms`, `--vad-max-segment-ms`,
  `--vad-noise-multiplier` (prima erano fissi).

## 10. Flush WAV a timer — `src/storage.rs`, `src/session.rs`

- Rimosso il `flush` a ogni chunk (20 ms).
- Ora header WAV + campioni in buffer vengono checkpointati con lo stesso timer di
  `status.json` (~250 ms, ~4 volte/s) tramite `flush_audio()`.
- Su crash si perde al massimo l'ultimo intervallo; niente più riscrittura
  dell'header decine di volte al secondo.

## 11. `stop` basato sul progresso — `src/control.rs`

- Prima: attesa massima fissa di 60 s.
- Ora: 300 s di **inattività**, azzerati ogni volta che il transcript cresce.
  Una finalizzazione/replay lunga può concludersi, un vero blocco fallisce.
- Poll ogni 200 ms.

## 12. `repair_jsonl_tail` a finestre — `src/storage.rs`

- Prima: leggeva tutto il file in memoria per trovare la coda incompleta.
- Ora: apre in lettura/scrittura e cerca l'ultimo `\n` leggendo all'indietro solo
  finestre da 64 KB. File mancante = niente da riparare (Ok, non errore).
- Adatto a transcript lunghi.

## 13. Normalizer condiviso — `src/audio.rs`, `src/live_audio.rs`

- Il resampler del percorso file è ora usato anche dal live (prima esistevano due
  copie). Stesso comportamento: cursore persistente, nessun click ai confini dei
  pacchetti.

## 14. Rifiniture

- `src/session.rs`: `recovery_status` → `initial_status`, un solo costruttore per
  lo stato iniziale.
- Documentazione: `docs/COMMANDS_HELP.md` e `docs/RELAZIONE.md` aggiornati;
  rimossi dal README i link a file inesistenti (`PROJECT_CONTEXT.md`,
  `ROADMAP.md`); `TODO.txt` eliminato.

## Problemi aperti (dal code review, non ancora corretti)

1. **Isteresi invertita con `--vad-noise-multiplier < 1`** (`src/segment.rs`):
   la soglia di chiusura può superare quella di apertura. Validare `>= 1.0`.
2. **`stop` senza tetto assoluto** (`src/control.rs`): con transcript in continua
   crescita l'attesa non termina mai. Aggiungere un cap massimo.
3. **Doc contraddittoria** (`docs/RELAZIONE.md`): dice che un export di "altra
   versione" è rifiutato, ma ora sono rifiutate solo le versioni più nuove.
4. `repair_jsonl_tail` richiede permesso di scrittura anche quando non c'è nulla
   da riparare (`src/storage.rs`).
5. `doctor` verifica il modello di default anche su terminale, dove `start`
   potrebbe chiederne un altro (`src/cli.rs`).
6. L'accettazione di transcript v1–v3 non è coperta da test (`src/export.rs`).
