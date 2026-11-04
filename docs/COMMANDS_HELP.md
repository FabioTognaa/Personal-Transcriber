# Comandi e opzioni

Riferimento completo della riga di comando di `scrib`. Elenca
ogni comando e ogni opzione con valore, default e vincoli, così il README può
restare concettuale e rimandarli qui.

## Sintassi generale

```text
scrib <comando> [opzioni]
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
indirizzano una sessione (`start`, `transcribe`, `doctor`, `status`, `pause`,
`resume`, `stop`). `devices` ed `export` non lo espongono.

## Opzioni condivise

Alcune opzioni sono definite una volta e riusate su più comandi. Le opzioni del
**modello** e della **lingua** valgono per `start`, `transcribe` e `doctor`; le
opzioni **VAD** valgono solo per `start`; quelle di **inferenza** valgono per
`start` e `transcribe`.

### Modello e lingua

| Opzione                 | Tipo         | Default | Descrizione                                                                                                                    |
| ----------------------- | ------------ | ------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `--model <PERCORSO>`    | percorso     | —       | File GGML singolo oppure directory bundle Parakeet. Ha la precedenza su `--model-preset`.                                      |
| `--model-preset <NOME>` | enum         | —       | Modello documentato risolto sotto `models/` (o `SCRIB_MODELS_DIR`). Valori: `medium`, `large-v3-turbo`, `large-v3`, `parakeet-v3`. |
| `--language <LINGUA>`   | `it` \| `en` | `it`    | Lingua parlata. Per Parakeet è solo dichiarativa: il modello rileva la lingua da solo.                                         |
| `--asr-prompt <TESTO>`  | testo        | prompt predefinito della lingua (solo Whisper) | Prompt iniziale per Whisper. `--asr-prompt ""` lo disattiva. Parakeet lo ignora.                           |

Non esiste un modello predefinito. Se non si passa né `--model` né
`--model-preset` e si lancia `start` o `transcribe` da un terminale interattivo,
compare un menu con i **soli modelli presenti su disco**, ordinati per
dimensione. Se ce n'è uno solo viene scelto senza domanda. Senza terminale e
senza modello il comando fallisce con un errore esplicito. Dal menu, `Esc` o
`Ctrl-C` annulla la scelta e non avvia nulla.

Preset e artefatti documentati:

| Preset           | Artefatto                                      | Dimensione |
| ---------------- | ---------------------------------------------- | ---------- |
| `medium`         | `models/ggml-medium-q5_0.bin`                  | ~540 MB    |
| `large-v3-turbo` | `models/ggml-large-v3-turbo-q8_0.bin`          | ~874 MB    |
| `large-v3`       | `models/ggml-large-v3-q5_0.bin`                | ~1,1 GB    |
| `parakeet-v3`    | `models/parakeet-tdt-0.6b-v3-onnx/` (directory) | ~670 MB    |

Parakeet è una directory con più file ONNX (licenza CC-BY-4.0). Per scaricare
solo la variante int8:

```sh
mkdir -p models/parakeet-tdt-0.6b-v3-onnx
for f in config.json vocab.txt nemo128.onnx \
         encoder-model.int8.onnx decoder_joint-model.int8.onnx; do
  curl -fL \
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/$f" \
    -o "models/parakeet-tdt-0.6b-v3-onnx/$f"
done
```

Parakeet è incluso nella build standard.

### Segmentazione vocale (VAD)

Flag del solo `start`: il percorso batch di `transcribe` non usa il VAD e lascia
la segmentazione a Whisper. Di seguito le impostazioni di default:

| Opzione                         | Tipo     | Default | Descrizione                                             |
| ------------------------------- | -------- | ------- | ------------------------------------------------------- |
| `--vad-threshold <NUMERO>`      | decimale | `0.02`  | Soglia RMS oltre la quale un chunk è voce.              |
| `--vad-start-ms <MILLISECONDI>` | intero   | `60`    | Voce consecutiva necessaria ad aprire un segmento.      |
| `--vad-end-ms <MILLISECONDI>`   | intero   | `800`   | Silenzio consecutivo necessario a chiudere un segmento. |

### Inferenza

L'unico flag dispnibile è quello della flash attention attivato di default. Per controllare se l'accuraezza migliora a scapito di un esosità di memorie e tempo vi consiglio di fare delle proveComparative. Parakeet ignora la flash attention e gli altri parametri di decoding di Whisper.

| Opzione                               | Tipo     | Default | Descrizione                                                                                               |
| ------------------------------------- | -------- | ------- | --------------------------------------------------------------------------------------------------------- |
| `--asr-flash-attention <true\|false>` | booleano | `true`  | Attiva flash attention di whisper.cpp. Va passato un valore esplicito, es. `--asr-flash-attention false`. |

## Comandi

### `devices`

```text
scrib devices
```

Elenca in JSON i dispositivi audio visibili, con direzione supportata e
configurazione predefinita. Non apre alcuno stream. Nessuna opzione oltre a
quelle globali.

### `doctor`

```text
scrib doctor [opzioni]
```

Verifica i prerequisiti locali senza modificare la configurazione di macOS.
Restituisce un report JSON; se un prerequisito manca, `ready` è `false` e
l'exit code è diverso da zero.

| Opzione                     | Tipo     | Default           | Descrizione                                                                                            |
| --------------------------- | -------- | ----------------- | ------------------------------------------------------------------------------------------------------ |
| `--model <PERCORSO>`        | percorso | —                 | Modello da verificare (file o directory). Ha la precedenza su `--model-preset`.                        |
| `--model-preset <NOME>`     | enum     | —                 | Preset da verificare se `--model` non è passato. Senza nessuno dei due, `doctor` fallisce.             |
| `--probe-audio`             | flag     | spento            | Apre entrambi gli input live per tre secondi e verifica che arrivi segnale, senza creare una sessione. |
| `--microphone <NOME>`       | testo    | input predefinito | Microfono usato dal probe. Richiede `--probe-audio`.                                                   |
| `--system-audio <NOME>`     | testo    | BlackHole         | Input di sistema usato dal probe. Richiede `--probe-audio`.                                            |
| `--sessions-dir <PERCORSO>` | percorso | `sessions`        | Directory delle sessioni da verificare.                                                                |

Senza `--probe-audio`, permesso del microfono e routing reale restano non
verificati e il report contiene un avviso.

### `start`

```text
scrib start [opzioni]
```

| Opzione                        | Tipo         | Default            | Descrizione                          |
| ------------------------------ | ------------ | ------------------ | ------------------------------------ |
| `--microphone <NOME>`          | testo        | input predefinito  | Dispositivo del microfono.           |
| `--system-audio <NOME>`        | testo        | BlackHole rilevato | Dispositivo dell'audio di sistema.   |
| `--silence-timeout <MINUTI>`   | intero 1–30  | `15`               | Chiude la sessione dopo N minuti di silenzio consecutivo. |
| `--model <PERCORSO>`           | percorso     | —                  | Vedi “Modello e lingua”.             |
| `--model-preset <NOME>`        | enum         | —                  | Vedi “Modello e lingua”.             |
| `--language <LINGUA>`          | `it` \| `en` | `it`               | Vedi “Modello e lingua”.             |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua (solo Whisper) | Vedi “Modello e lingua”.             |
| `--vad-threshold <NUMERO>`     | decimale     | `0.02`             | Vedi “Segmentazione vocale”.         |
| `--vad-start-ms <MS>`          | intero       | `60`               | Vedi “Segmentazione vocale”.         |
| `--vad-end-ms <MS>`            | intero       | `800`              | Vedi “Segmentazione vocale”.         |
| `--asr-flash-attention <BOOL>` | booleano     | `true`             | Vedi “Inferenza”.                    |
| `--sessions-dir <PERCORSO>`    | percorso     | `sessions`         | Directory in cui creare la sessione. |

Su un terminale `start` chiede il nome della sessione e crea `YY-MM-DD_<nome>`:
gli spazi diventano `_`, i caratteri `/ \ : * ? " < > |` e i nomi già usati sono
rifiutati, `q` annulla. Senza terminale interattivo resta il nome `YY-MM-DD`.

Con `--silence-timeout` la sessione si chiude da sola dopo N minuti consecutivi di
silenzio. Il conteggio è in tempo audio e non si ferma con `pause`: la chiusura è
la stessa di `stop` (stato `completed`) e l'evento `silence_timeout` in
`events.jsonl` ne conserva il motivo.

### `transcribe`

```text
scrib transcribe [FILE] [opzioni]
```

Trascrive un file audio esistente con un percorso batch dedicato: decodifica il
file in streaming e fa **una sola** inferenza sull'intero audio, lasciando a
Whisper la segmentazione nativa. Non usa il VAD del live e non conserva l'audio.
I formati supportati sono `.wav`, `.mp3`, `.m4a`, `.mp4` e `.aac`; il vecchio
limite di 64 MiB / 30 minuti per i WAV non esiste più.

In `sessions/` nasce una cartella nuova (stesso nome `YY-MM-DD` di `start`) con:

```text
session.json        lingua, modello, SHA-256, parametri, file di origine
transcript.jsonl    la fonte canonica, un segmento per riga
<nome>.txt          una riga per gruppo di segmenti (max 20 s), con i timestamp
```

Non ci sono `audio/`, `status.json`, `events.jsonl` né `current-session.json`:
la cartella non è una sessione live e `status`, `pause`, `resume` e `stop` non la
vedono. La directory viene creata solo a trascrizione finita; se la scrittura
fallisce viene rimossa. Il comando stampa la directory, non il path del `.txt`.
Per il testo senza timestamp:

```sh
scrib export sessions/<cartella> --format text
```

L'audio decodificato resta in memoria. Il limite è di 4 ore: oltre, il comando
si ferma con un errore chiaro prima di chiamare il modello. Gli altri formati
sono decodificati in streaming e non hanno il vecchio cap.

Su un terminale mostra una barra di avanzamento su una riga sola: la percentuale
già trascritta sul totale, non il tempo rimanente. Con l'output rediretto non
stampa nulla. I log di whisper.cpp/ggml vanno su standard error e compaiono solo
con `-v`.

| Argomento/Opzione              | Tipo         | Default            | Descrizione                                    |
| ------------------------------ | ------------ | ------------------ | ---------------------------------------------- |
| `<FILE>` (argomento)           | percorso     | —                  | Input `.wav`, `.mp3`, `.m4a`, `.mp4` o `.aac`; chiesto su terminale se omesso. |
| `--model <PERCORSO>`           | percorso     | —                  | Vedi “Modello e lingua”.                       |
| `--model-preset <NOME>`        | enum         | —                  | Vedi “Modello e lingua”.                       |
| `--language <LINGUA>`          | `it` \| `en` | `it`               | Vedi “Modello e lingua”.                       |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua (solo Whisper) | Vedi “Modello e lingua”.                       |
| `--asr-flash-attention <BOOL>` | booleano     | `true`             | Vedi “Inferenza”.                              |
| `--sessions-dir <PERCORSO>`    | percorso     | `sessions`         | Directory in cui creare la cartella di output. |

Con Parakeet i segmenti sono i chunk interni da 15 secondi: i timestamp sono
reali ma possono spezzare una frase.

## Comandi per una sessione in atto

Di seguito sono riporati i comandi da poter eseguire per monitorare e gestire la sessione di transcrizione attuale. Occorre che venga lanciato un secondo terminale in parallelo con quello della sessione corrente in atto.

### `status`

```text
scrib status [opzioni]
```

Stampa il report JSON della sessione corrente: metadati, stato, durata,
profondità delle code, ritardo rispetto all'ultimo segmento ASR e percorsi dei
file.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `pause`

```text
scrib pause [opzioni]
```

Sospende la trascrizione lasciando attiva la registrazione. Chiude il segmento
aperto e non attraversa l'intervallo sospeso. In questo modo la trascrizione subisce un taglio fino al lancio di un eventuale `resume`.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `resume`

```text
scrib resume [opzioni]
```

Riprende la trascrizione dopo una `pause`.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `stop`

```text
scrib stop [opzioni]
```

Finalizza la sessione attiva, questo è il comando che deve essere lanciato per chiudere la registrazione e completare la crezione della cartella di sessione. Attende il completamento. Su una sessione già
`completed` restituisce lo stato senza rifare il lavoro; su una `failed` i
comandi di controllo sono rifiutati.

| Opzione                     | Tipo     | Default    | Descrizione                                  |
| --------------------------- | -------- | ---------- | -------------------------------------------- |
| `--sessions-dir <PERCORSO>` | percorso | `sessions` | Directory che contiene la sessione corrente. |

### `export`

```text
scrib export <SESSIONE> [opzioni]
```

Rilegge il file `transcript.jsonl` di una sessione (anche di una cartella creata
da `transcribe`) e restituisce il testo nel formato scelto, senza modificarlo.
Rifiuta come destinazione qualsiasi file canonico della sessione.

| Argomento/Opzione           | Tipo     | Default         | Descrizione                                                                 |
| --------------------------- | -------- | --------------- | --------------------------------------------------------------------------- |
| `<SESSIONE>` (argomento)    | percorso | richiesto       | Directory della sessione o della trascrizione.                              |
| `--format <FORMATO>`        | enum     | `markdown`      | `jsonl`, `text`, `markdown`, `srt` o `vtt`. `text` è il testo puro senza timestamp. |
| `-o`, `--output <PERCORSO>` | percorso | standard output | File di destinazione. La scrittura è atomica.                               |
| `--allow-partial`           | flag     | spento          | Consente l'export di una sessione non completata (fallita o ancora attiva). |

## Esempio di una pipeline di utilizzo

```sh
scrib devices
scrib doctor --probe-audio
scrib start --language it
scrib start --model-preset large-v3-turbo --language it
scrib transcribe riunione.mp3 --language it
scrib status
scrib pause
scrib resume
scrib stop
scrib export sessions/<sessione> --format srt --output transcript.srt
```
