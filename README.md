#

Welcome to my personal transcriber!

I decided to write this program in order to build a personal tool which was useful, functional and istructive at the same time.

### Disclaimer:

The entire codebase it's been writing using an agentic AI: expecially by Sol 5.6 and Grok 4.7 for documentation.
L'obiettivo principale era quello di esercitarmi nell'usare AI agentiche per scrivere prodotti software in modo efficace.

Si tratta di un programma utilizzabile da terminale, che si collega all'audio del microfono e quello del dispositivo audio del pc tramite un proxy ( es. BlackHole per MacOS), producendo un elaborato testuale di quello che è stato registrato.  
L'intera pipeline di lavoro si svolge in locale, così come la conservazione di ogni elaborato, salvaguardando privacy e sicurezza.

# Installazioni in base al tuo Sistema Operativo

Attualmente il programma e' in fase di pre-release ed è destinata a macOS Apple Silicon, in arrivo versioni compatibili anche per altri sistemi operativi...

### [Installazione per MacOS Apple Silicon](docs/INSTALLATION_MACOS.md)

### [Installazione per Windows](docs/INSTALLATION_WINDOWS.md)

### [Installazione per Linux](docs/INSTALLATION_LINUX.md)

# Come posso usare questo programma?

## Modelli e motori

Scribroom supporta due motori, scelti indirettamente dal modello:

- **Whisper** (`whisper.cpp`): file GGML singoli; supporta prompt e glossario.
- **Parakeet TDT 0.6B v3** (ONNX Runtime): directory bundle, 25 lingue europee,
  punteggiatura nativa, nessun prompt.

Non esiste un modello predefinito: `start`, `transcribe` e `doctor` richiedono
`--model`, `--model-preset`, oppure una scelta dal menu interattivo. Il menu
mostra solo i modelli già presenti su disco e li ordina per dimensione; se non
ce ne sono, il comando spiega come scaricarli.

## Live Recording

Funzione principale del programma, nella quale l'utente può registrare l'audio dal proprio microfono e dall'audio del sistema contemporaneamente, mentre ad esempio si è in una riunione su Teams, Zoom, Google Meet, ecc.

### Fase di Preparazione

Questa è la fase iniziale del progetto, nella quale il programma si prepara ad avviare la sessione.

Come prima cosa viene **parsata la CLI** per tradurre la riga di comando e trasformarla in una struct Rust tipizzata. Il lavoro di parsing viene svolto interamente dalla libreria **clap**.
In questo modo vengono tradotti i comandi in stringhe e setta le opzioni di default (a seconda dei flag inseriti in CLI)

Viene settato il logging del transcriber una volta in esecuzione ( flag corrispndente: `-v` e ripetibile) per consentire di capire cosa sta succedendo se ci sono errori, ecc.

### Installazione dei Segnali

Vengono installate le callbackk per la cattura dei segnali SIGINT e SIGTERM: in questo modo si può comunicare con il processo in modo sicuro tramite questi comandi.
Questi segnali vengono registrati solo al momento del comando `start`.

In questo fragente vengono validate anche le potenziali opzioni di alcune flag, come la lingua del parlato, verifica che il modello scelto esista e viene calcolata l'identità del modello.
Quest'ultima viene calcolata tramite l'algoritmo SHA-256 e viene memorizzata in un file .sha256 per non dover ricalcolare l'identità del modello ogni volta. È utile per riconsocere quale modello ha prodotto un eventuale transcript e per fare in modo che il comando `doctor` si a certo di quale modello sia caricabile e che non venga scambiato per un altro.

Viene poi **caricato il motore dedotto dall'artefatto scelto**:

**Whisper** è una rete neurale di tipo Transformer - creata da OpenAI - che svolge la funzione di encoder-decoder per riconoscere il parlato da uno spettrogramma. I file dei pesi sono disponibili nella cartella **/models** e girano tramite **Whisper.cpp** (whisper-rs).

**Parakeet** è un modello NVIDIA (NeMo, TDT) eseguito tramite ONNX Runtime. Vive in una directory con più file ONNX ed è incluso nella build standard; la feature `parakeet` resta solo come uscita di sicurezza per compilare senza ONNX Runtime.

Il motore viene dato ad un thread worker, che a seconda del compito lo usa liberamente.

- valida la --sessions-dir; prende il lock esclusivo sessions/.runner.lock (un solo start alla volta); recupera una sessione rimasta running da un crash precedente marcandola failed.
- risolve la sorgente: microfono = input di default (o --microphone); sistema = BlackHole (o --system-audio); i due devono essere dispositivi diversi.
- crea il Segmenter, controlla lo spazio disco (live: ≥ 64 MiB, ricontrollo ogni 5 s).
- crea sessions/YY-MM-DD/ (giorno locale, \_2, \_3… se collide) con session.json, status.json, events.jsonl, transcript.jsonl, audio/{mixed,microphone,system}.wav.
- sink.start() avvia il thread worker ASR (apre transcript.jsonl).
- scrive l'evento session_started, stato running.

-------stcrivere come si usa il prodotto introducendo i comandi ed i loro flag via via

## Transcrizione di file

Possono essere trascritti anche file audio già esistenti nella propria macchina:
in questo caso la trascrizione non simula lo streaming del live e usa un percorso
batch dedicato. I formati supportati sono `wav`, `mp3`, `m4a`, `mp4` e `aac`;
tutti vengono decodificati in streaming, quindi non esiste più il vecchio limite
di 64 MiB / 30 minuti per i WAV.

Il batch decodifica il file e fa **una sola** inferenza sull'intero audio,
lasciando a Whisper la segmentazione nativa. Il tempo dipende quindi dalla durata
dell'audio, non dal numero di frasi. L'audio decodificato resta in memoria: il
limite è fissato a 4 ore, oltre le quali il comando si ferma con un errore chiaro.

Ogni esecuzione crea una cartella nuova sotto `sessions/` con:

- `session.json`: lingua, modello, SHA-256, parametri e file di origine;
- `transcript.jsonl`: la fonte canonica, un segmento per riga;
- `<nome>.txt`: una riga per gruppo di segmenti (max 20 s), con i timestamp.

Per ottenere lo stesso testo senza timestamp:

```sh
scrib export sessions/<cartella> --format text
```

Con Parakeet i segmenti sono i chunk interni da 15 secondi: i timestamp sono
reali ma possono spezzare una frase.

## Limiti noti

- La v1 supporta macOS su Apple Silicon; Windows e Linux non sono supportati.
- La lingua deve essere scelta esplicitamente tra italiano e inglese; non esiste
  rilevamento automatico. Per Parakeet `--language` è solo dichiarativa: il
  modello rileva la lingua da solo.
- Parakeet ignora il prompt/glossario e i parametri di decoding di Whisper.
- Non sono incluse diarizzazione, identificazione degli speaker, UI o funzioni cloud.
- I file delle sessioni non sono cifrati a riposo.
- Il rilevamento del parlato usa una soglia energetica RMS, non un modello VAD.
- La cattura live non è ancora stata validata end-to-end su una call reale.
- `transcribe` elabora un file alla volta e in un solo thread: non esistono
  pause o ripresa. Su un terminale mostra una barra di avanzamento; con l'output
  rediretto non stampa nulla finché non ha finito.

## Documentazione nel dettaglio

- [Sezione aiuto per tutti i comandi](docs/COMMANDS_HELP.md)
- [Funzionamento e architettura](docs/RELAZIONE.md)
- [Piano Parakeet](docs/PARAKEET_PLAN.md)
- [Glossario del dominio](GLOSSARY.md)
- [Decisioni architetturali](docs/adr/)
- [Come leggere una cartella di sessione](sessions/README.md)
- [Storico delle modifiche](CHANGELOG.md)

## Sviluppo locale

I controlli della milestone corrente sono:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Il test con un modello Whisper reale è ignorato per impostazione predefinita e
richiede `PERSONAL_TRANSCRIBER_MODEL`:

```sh
PERSONAL_TRANSCRIBER_MODEL=models/ggml-large-v3-turbo-q8_0.bin \
  cargo test real_whisper_model_processes_local_audio -- --ignored
```

Il test Parakeet è ignorato per impostazione predefinita e richiede il bundle
nella cartella dei modelli:

```sh
cargo test --lib parakeet -- --ignored
```

## Privacy e consenso

Prima di registrare o trascrivere una riunione, è buona norma informare i partecipanti e verificare le
politiche aziendali e gli obblighi applicabili. Il fatto che l'elaborazione sia locale
non elimina questo requisito. Ogni responsabilità è a carico di chi usufruisce del programma

## Licenza

MIT. Vedi [LICENSE](LICENSE).
