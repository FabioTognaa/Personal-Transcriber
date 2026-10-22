# Comandi e opzioni

Riferimento completo della riga di comando di `personal-transcriber`. Elenca
ogni comando e ogni opzione con valore, default e vincoli, così il README può
restare concettuale e rimandarli qui.

## Sintassi generale

I comandi seguenti presuppongono che `personal-transcriber` sia installato nel
sistema con:

```text
cargo install --path .
```

È possibile disinstallarlo in ogni momento con:

```text
cargo uninstall <nome-package>.
```

In alternativa si possono seguire i comandi di questa sezione senza installare
`personal-transcriber`, eseguendo direttamente il binario compilato.

```text
personal-transcriber <comando> [opzioni]
```

Il binario esegue un solo comando per invocazione e resta in primo piano fino
alla fine. Su errore stampa `error: <messaggio>` ed esce con codice diverso da
zero.

## Opzioni globali

Disponibili su tutti i comandi.

| Opzione           | Tipo                  | Default | Descrizione                                                                          |
| ----------------- | --------------------- | ------- | ------------------------------------------------------------------------------------ |
| `-v`, `--verbose` | conteggio, ripetibile | 0       | Alza il dettaglio dei log diagnostici su standard error. Ripetibile (`-vv`, `-vvv`). |
| `-h`, `--help`    | flag                  | —       | Mostra l'aiuto del comando e delle sue opzioni.                                      |
| `-V`, `--version` | flag                  | —       | Stampa la versione del programma.                                                    |

`--sessions-dir` non è globale: compare solo sui comandi che creano o
indirizzano una sessione (`start`, `doctor`, `transcribe`, `status`, `pause`,
`resume`, `stop`). `devices` ed `export` non lo espongono.

## Opzioni condivise

Alcune opzioni sono definite una volta e riusate su più comandi. Le opzioni del
**modello** e della **lingua** valgono per `start`, `transcribe` e `doctor`; le
opzioni **VAD** e di **inferenza** valgono per `start` e `transcribe`.

### Modello e lingua

| Opzione                 | Tipo         | Default                         | Descrizione                                                                                           |
| ----------------------- | ------------ | ------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `--model <PERCORSO>`    | percorso     | —                               | File GGML locale. Ha la precedenza su `--model-preset`.                                               |
| `--model-preset <NOME>` | enum         | `large-v3-turbo`                | Modello documentato risolto sotto `models/`. Valori: `small`, `medium`, `large-v3-turbo`, `large-v3`. |
| `--language <LINGUA>`   | `it` \| `en` | `it`                            | Lingua parlata, valida per tutta la sessione o per l'intero file. Nessun rilevamento automatico.      |
| `--asr-prompt <TESTO>`  | testo        | prompt predefinito della lingua | Prompt iniziale per Whisper. `--asr-prompt ""` lo disattiva.                                          |

Se non si passa né `--model` né `--model-preset` e si lancia `start` da un
terminale interattivo, compare un menu dei modelli; senza terminale (script, CI,
test) il menu non compare e vale `large-v3-turbo`. Dal menu, `q` annulla la scelta e non
avvia nulla.

Preset e file documentati:

| Preset           | File                                  | Dimensione |
| ---------------- | ------------------------------------- | ---------- |
| `small`          | `models/ggml-small-q5_1.bin`          | ~190 MB    |
| `medium`         | `models/ggml-medium-q5_0.bin`         | ~540 MB    |
| `large-v3-turbo` | `models/ggml-large-v3-turbo-q8_0.bin` | ~874 MB    |
| `large-v3`       | `models/ggml-large-v3-q5_0.bin`       | ~1,1 GB    |

### Segmentazione vocale (VAD)

Flag per impsotare i timing del riconscimento vocale, di seguito le impostazioni di default:

| Opzione                               | Tipo     | Default | Descrizione                                                                       |
| ------------------------------------- | -------- | ------- | --------------------------------------------------------------------------------- |
| `--vad-threshold <NUMERO>`            | decimale | `0.02`  | Soglia RMS oltre la quale un chunk è voce.                                        |
| `--vad-noise-multiplier <NUMERO>`     | decimale | `4.0`   | Quanto la voce deve superare il rumore di fondo stimato per contare come parlato. |
| `--vad-start-ms <MILLISECONDI>`       | intero   | `60`    | Voce consecutiva necessaria ad aprire un segmento.                                |
| `--vad-end-ms <MILLISECONDI>`         | intero   | `1600`  | Silenzio consecutivo necessario a chiudere un segmento.                           |
| `--vad-pre-roll-ms <MILLISECONDI>`    | intero   | `200`   | Audio tenuto prima della prima voce.                                              |
| `--vad-post-roll-ms <MILLISECONDI>`   | intero   | `200`   | Audio tenuto dopo l'ultima voce.                                                  |
| `--vad-min-speech-ms <MILLISECONDI>`  | intero   | `100`   | Sotto questa durata il segmento è scartato (contato in `status.json`).            |
| `--vad-max-segment-ms <MILLISECONDI>` | intero   | `30000` | Tetto di durata; il parlato più lungo è tagliato sulla pausa più lunga.           |

### Inferenza

L'unico flag per la flash attention è attivato di default; `--asr-context` è
spento di default. Per controllare se l'accuraezza migliora a scapito di un esosità di memorie e tempo vi consiglio di fare delle proveComparative.

| Opzione                               | Tipo     | Default | Descrizione                                                                                               |
| ------------------------------------- | -------- | ------- | --------------------------------------------------------------------------------------------------------- |
| `--asr-flash-attention <true\|false>` | booleano | `true`  | Attiva flash attention di whisper.cpp. Va passato un valore esplicito, es. `--asr-flash-attention false`. |
| `--asr-context`                       | flag     | spento  | Porta la coda del segmento precedente nel prompt iniziale (rischio di ripetere un errore precoce).        |

## Comandi

### `devices`

```text
personal-transcriber devices
```

Elenca in JSON i dispositivi audio visibili, con direzione supportata e
configurazione predefinita. Non apre alcuno stream. Nessuna opzione oltre a
quelle globali.

### `doctor`

```text
personal-transcriber doctor [opzioni]
```

Verifica i prerequisiti locali senza modificare la configurazione di macOS.
Restituisce un report JSON; se un prerequisito manca, `ready` è `false` e
l'exit code è diverso da zero.

| Opzione                     | Tipo     | Default                 | Descrizione                                                                                            |
| --------------------------- | -------- | ----------------------- | ------------------------------------------------------------------------------------------------------ |
| `--model <PERCORSO>`        | percorso | preset `large-v3-turbo` | Modello da verificare. Ha la precedenza su `--model-preset`.                                           |
| `--model-preset <NOME>`     | enum     | `large-v3-turbo`        | Preset da verificare se `--model` non è passato.                                                       |
| `--probe-audio`             | flag     | spento                  | Apre entrambi gli input live per tre secondi e verifica che arrivi segnale, senza creare una sessione. |
| `--microphone <NOME>`       | testo    | input predefinito       | Microfono usato dal probe. Richiede `--probe-audio`.                                                   |
| `--system-audio <NOME>`     | testo    | BlackHole               | Input di sistema usato dal probe. Richiede `--probe-audio`.                                            |
| `--sessions-dir <PERCORSO>` | percorso | `sessions`              | Directory delle sessioni da verificare.                                                                |

Senza `--probe-audio`, permesso del microfono e routing reale restano non
verificati e il report contiene un avviso.

### `start`

```text
personal-transcriber start [opzioni]
```

| Opzione                        | Tipo         | Default            | Descrizione                          |
| ------------------------------ | ------------ | ------------------ | ------------------------------------ |
| `--microphone <NOME>`          | testo        | input predefinito  | Dispositivo del microfono.           |
| `--system-audio <NOME>`        | testo        | BlackHole rilevato | Dispositivo dell'audio di sistema.   |
| `--silence-timeout <MINUTI>`   | intero 1–30  | `15`               | Chiude la sessione dopo N minuti di silenzio consecutivo. |
| `--model <PERCORSO>`           | percorso     | —                  | Vedi “Modello e lingua”.             |
| `--model-preset <NOME>`        | enum         | `large-v3-turbo`   | Vedi “Modello e lingua”.             |
| `--language <LINGUA>`          | `it` \| `en` | `it`               | Vedi “Modello e lingua”.             |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua | Vedi “Modello e lingua”.             |
| `--vad-threshold <NUMERO>`     | decimale     | `0.02`             | Vedi “Segmentazione vocale”.         |
| `--vad-start-ms <MS>`          | intero       | `60`               | Vedi “Segmentazione vocale”.         |
| `--vad-end-ms <MS>`            | intero       | `1600`             | Vedi “Segmentazione vocale”.         |
| `--asr-flash-attention <BOOL>` | booleano     | `true`             | Vedi “Inferenza”.                    |
| `--sessions-dir <PERCORSO>`    | percorso     | `sessions`         | Directory in cui creare la sessione. |

Con `--silence-timeout` la sessione si chiude da sola dopo N minuti consecutivi di
silenzio. Il conteggio è in tempo audio e non si ferma con `pause`: vale la soglia
`--vad-threshold` sul mix, la chiusura è la stessa di `stop` (stato `completed`) e
l'evento `silence_timeout` in `events.jsonl` ne conserva il motivo. Un valore fuori
da 1–30 o non intero viene rifiutato da clap con un errore in inglese.

### `transcribe`

```text
personal-transcriber transcribe [FILE] [opzioni]
```

Trascrive un file audio esistente, senza registrare e senza conservare audio: in
`sessions/` nasce una cartella nuova (stesso nome `YY-MM-DD` di `start`) con dentro
solo `<nome>.txt`. Non sovrascrive mai nulla, perché la cartella è nuova; se la
trascrizione fallisce non resta niente sul disco. Elabora un solo file per
invocazione, in un unico thread. Se `FILE` è omesso, il percorso viene richiesto su
un terminale interattivo.

Su un terminale mostra una barra di avanzamento su una riga sola: la percentuale
già trascritta sul totale, non il tempo rimanente. Con l'output rediretto non
stampa nulla. I log di whisper.cpp/ggml vanno su standard error e compaiono solo
con `-v`.

| Argomento/Opzione              | Tipo         | Default                                      | Descrizione                                    |
| ------------------------------ | ------------ | -------------------------------------------- | ---------------------------------------------- |
| `<FILE>` (argomento)           | percorso     | opzionale (richiesto su terminale se omesso) | Input `.wav`, `.mp3`, `.m4a`, `.mp4` o `.aac`. |
| `--model <PERCORSO>`           | percorso     | —                                            | Vedi “Modello e lingua”.                       |
| `--model-preset <NOME>`        | enum         | `large-v3-turbo`                             | Vedi “Modello e lingua”.                       |
| `--language <LINGUA>`          | `it` \| `en` | `it`                                         | Vedi “Modello e lingua”.                       |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua                           | Vedi “Modello e lingua”.                       |
| `--vad-threshold <NUMERO>`     | decimale     | `0.02`                                       | Vedi “Segmentazione vocale”.                   |
| `--vad-start-ms <MS>`          | intero       | `60`                                         | Vedi “Segmentazione vocale”.                   |
| `--vad-end-ms <MS>`            | intero       | `1600`                                       | Vedi “Segmentazione vocale”.                   |
| `--asr-flash-attention <BOOL>` | booleano     | `true`                                       | Vedi “Inferenza”.                              |
| `--sessions-dir <PERCORSO>`    | percorso     | `sessions`                                   | Directory in cui creare la cartella di output. |

I WAV passano dal lettore in memoria, quindi ereditano il limite di 64 MiB e 30
minuti; gli altri formati sono decodificati in streaming e non hanno tale
limite.

## Comandi per una sessione in atto

Di seguito sono riporati i comandi da poter eseguire per monitorare e gestire la sessione di transcrizione attuale. Occorre che venga lanciato un secondo terminale in parallelo con quello della sessione corrente in atto.

### `status`

```text
personal-transcriber status [opzioni]
```

Stampa il report JSON della sessione corrente: metadati, stato, durata,
profondità delle code, ritardo rispetto all'ultimo segmento ASR e percorsi dei
file.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `pause`

```text
personal-transcriber pause [opzioni]
```

Sospende la trascrizione lasciando attiva la registrazione. Chiude il segmento
aperto e non attraversa l'intervallo sospeso. In questo modo la trascrizione subisce un taglio fino al lancio di un eventuale `resume`.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `resume`

```text
personal-transcriber resume [opzioni]
```

Riprende la trascrizione dopo una `pause`.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `stop`

```text
personal-transcriber stop [opzioni]
```

Finalizza la sessione attiva, questo è il comando che deve essere lanciato per chiudere la registrazione e completare la crezione della cartella di sessione. Attende il completamento. Su una sessione già
`completed` restituisce lo stato senza rifare il lavoro; su una `failed` i
comandi di controllo sono rifiutati.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `export`

```text
personal-transcriber export <SESSIONE> [opzioni]
```

Rilegge il file `transcript.jsonl` di una sessione e restituisce il testo in formato .txt e .md su console, senza
modificarlo. Rifiuta come destinazione qualsiasi file canonico della sessione.

| Argomento/Opzione           | Tipo     | Default         | Descrizione                                                                 |
| --------------------------- | -------- | --------------- | --------------------------------------------------------------------------- |
| `<SESSIONE>` (argomento)    | percorso | richiesto       | Directory della sessione.                                                   |
| `--format <FORMATO>`        | enum     | `markdown`      | `jsonl`, `text`, `markdown`, `srt` o `vtt`.                                 |
| `-o`, `--output <PERCORSO>` | percorso | standard output | File di destinazione. La scrittura è atomica.                               |
| `--allow-partial`           | flag     | spento          | Consente l'export di una sessione non completata (fallita o ancora attiva). |

## Comandi per lo sviluppo locale

Comandi da eseguire dalla radice del repository durante lo sviluppo.

- `cargo fmt --check` — verifica la formattazione senza modificare i file.
- `cargo fmt` — riformatta il codice sorgente.
- `cargo clippy --all-targets -- -D warnings` — esegue il linter su tutto il codice e tratta ogni avviso come errore.
- `cargo test` — esegue la suite di test.
- `cargo build` — compila la build non ottimizzata in `target/debug/`.
- `cargo build --release` — compila la build ottimizzata in `target/release/`.
- `cargo run -- <comando>` — esegue il binario compilato senza installarlo, es. `cargo run -- devices`.
- `cargo install --path .` — installa `personal-transcriber` nel sistema.
- `PERSONAL_TRANSCRIBER_MODEL=models/ggml-small-q5_1.bin cargo test real_whisper_model_processes_local_audio -- --ignored` — esegue il test con un modello Whisper reale, ignorato per impostazione predefinita.
- `cargo run --release --bin asr-benchmark -- <opzioni>` — esegue il benchmark ASR (opzioni in `benchmarks/README.md`).

controlli della milestone corrente sono:

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
cargo build --release
La build non ottimizzata finisce in target/debug/, quella ottimizzata in target/release/.

Il test con un modello Whisper reale è ignorato per impostazione predefinita e richiede PERSONAL_TRANSCRIBER_MODEL:

PERSONAL_TRANSCRIBER_MODEL=models/ggml-small-q5_1.bin \
 cargo test real_whisper_model_processes_local_audio -- --ignored
