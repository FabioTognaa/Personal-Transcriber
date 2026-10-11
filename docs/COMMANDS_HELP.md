# Comandi e opzioni

Riferimento completo della riga di comando di `personal-transcriber`. Elenca
ogni comando e ogni opzione con valore, default e vincoli, così il README può
restare concettuale e rimandarli qui.

## Sintassi generale

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
indirizzano una sessione (`start`, `doctor`, `status`, `pause`, `resume`,
`stop`). `devices`, `transcribe` ed `export` non lo espongono.

## Opzioni condivise

Alcune opzioni sono definite una volta e riusate su più comandi. Le opzioni del
**modello** e della **lingua** valgono per `start`, `transcribe` e `doctor`; le
opzioni **VAD** e di **inferenza** valgono per `start` e `transcribe`.

### Modello e lingua

| Opzione                 | Tipo         | Default                         | Descrizione                                                                                           |
| ----------------------- | ------------ | ------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `--model <PERCORSO>`    | percorso     | —                               | File GGML locale. Ha la precedenza su `--model-preset`.                                               |
| `--model-preset <NOME>` | enum         | `small`                         | Modello documentato risolto sotto `models/`. Valori: `small`, `medium`, `large-v3-turbo`, `large-v3`. |
| `--language <LINGUA>`   | `it` \| `en` | `it`                            | Lingua parlata, valida per tutta la sessione o per l'intero file. Nessun rilevamento automatico.      |
| `--asr-prompt <TESTO>`  | testo        | prompt predefinito della lingua | Prompt iniziale per Whisper. `--asr-prompt ""` lo disattiva.                                          |

Se non si passa né `--model` né `--model-preset` e si lancia `start` da un
terminale interattivo, compare un menu dei modelli; senza terminale (script, CI,
test) il menu non compare e vale `small`. Dal menu, `q` annulla la scelta e non
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

| Opzione                         | Tipo     | Default | Descrizione                                             |
| ------------------------------- | -------- | ------- | ------------------------------------------------------- |
| `--vad-threshold <NUMERO>`      | decimale | `0.02`  | Soglia RMS oltre la quale un chunk è voce.              |
| `--vad-start-ms <MILLISECONDI>` | intero   | `60`    | Voce consecutiva necessaria ad aprire un segmento.      |
| `--vad-end-ms <MILLISECONDI>`   | intero   | `800`   | Silenzio consecutivo necessario a chiudere un segmento. |

### Inferenza

L'unico flag dispnibile è quello della flash attention attivato di default. Per controllare se l'accuraezza migliora a scapito di un esosità di memorie e tempo vi consiglio di fare delle proveComparative.

| Opzione                               | Tipo     | Default | Descrizione                                                                                               |
| ------------------------------------- | -------- | ------- | --------------------------------------------------------------------------------------------------------- |
| `--asr-flash-attention <true\|false>` | booleano | `true`  | Attiva flash attention di whisper.cpp. Va passato un valore esplicito, es. `--asr-flash-attention false`. |

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

| Opzione                     | Tipo     | Default           | Descrizione                                                                                            |
| --------------------------- | -------- | ----------------- | ------------------------------------------------------------------------------------------------------ |
| `--model <PERCORSO>`        | percorso | preset `small`    | Modello da verificare. Ha la precedenza su `--model-preset`.                                           |
| `--model-preset <NOME>`     | enum     | `small`           | Preset da verificare se `--model` non è passato.                                                       |
| `--probe-audio`             | flag     | spento            | Apre entrambi gli input live per tre secondi e verifica che arrivi segnale, senza creare una sessione. |
| `--microphone <NOME>`       | testo    | input predefinito | Microfono usato dal probe. Richiede `--probe-audio`.                                                   |
| `--system-audio <NOME>`     | testo    | BlackHole         | Input di sistema usato dal probe. Richiede `--probe-audio`.                                            |
| `--sessions-dir <PERCORSO>` | percorso | `sessions`        | Directory delle sessioni da verificare.                                                                |

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
| `--model <PERCORSO>`           | percorso     | —                  | Vedi “Modello e lingua”.             |
| `--model-preset <NOME>`        | enum         | `small`            | Vedi “Modello e lingua”.             |
| `--language <LINGUA>`          | `it` \| `en` | `it`               | Vedi “Modello e lingua”.             |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua | Vedi “Modello e lingua”.             |
| `--vad-threshold <NUMERO>`     | decimale     | `0.02`             | Vedi “Segmentazione vocale”.         |
| `--vad-start-ms <MS>`          | intero       | `60`               | Vedi “Segmentazione vocale”.         |
| `--vad-end-ms <MS>`            | intero       | `800`              | Vedi “Segmentazione vocale”.         |
| `--asr-flash-attention <BOOL>` | booleano     | `true`             | Vedi “Inferenza”.                    |
| `--sessions-dir <PERCORSO>`    | percorso     | `sessions`         | Directory in cui creare la sessione. |

### `transcribe`

```text
personal-transcriber transcribe <FILE> [opzioni]
```

Trascrive un file audio esistente in `<nome>.txt` e `<nome>.md` accanto al path di input
all'input, senza creare una sessione e senza salvare audio. Se uno dei due file
di output esiste già, il comando si ferma senza sovrascriverlo. Elabora un solo
file per invocazione, in un unico thread.

| Argomento/Opzione              | Tipo         | Default            | Descrizione                                    |
| ------------------------------ | ------------ | ------------------ | ---------------------------------------------- |
| `<FILE>` (argomento)           | percorso     | richiesto          | Input `.wav`, `.mp3`, `.m4a`, `.mp4` o `.aac`. |
| `--model <PERCORSO>`           | percorso     | —                  | Vedi “Modello e lingua”.                       |
| `--model-preset <NOME>`        | enum         | `small`            | Vedi “Modello e lingua”.                       |
| `--language <LINGUA>`          | `it` \| `en` | `it`               | Vedi “Modello e lingua”.                       |
| `--asr-prompt <TESTO>`         | testo        | predefinito lingua | Vedi “Modello e lingua”.                       |
| `--vad-threshold <NUMERO>`     | decimale     | `0.02`             | Vedi “Segmentazione vocale”.                   |
| `--vad-start-ms <MS>`          | intero       | `60`               | Vedi “Segmentazione vocale”.                   |
| `--vad-end-ms <MS>`            | intero       | `800`              | Vedi “Segmentazione vocale”.                   |
| `--asr-flash-attention <BOOL>` | booleano     | `true`             | Vedi “Inferenza”.                              |

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

## Esempio di una pipeline di utilizzo

```sh
personal-transcriber devices
personal-transcriber doctor --probe-audio
personal-transcriber start --language it
personal-transcriber start --model-preset large-v3-turbo --language it
personal-transcriber transcribe riunione.mp3 --language @it
personal-transcriber status
personal-transcriber pause
personal-transcriber resume
personal-transcriber stop
personal-transcriber export sessions/<sessione> --format srt --output transcript.srt
```
