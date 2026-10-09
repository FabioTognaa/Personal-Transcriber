# personal-transcriber

Welcome to my personal transcriber!

I decided to write this program in order to build a personal tool which was useful, functional and istructive at the same time.

### Disclaimer:

The entire codebase it's been writing using an agentic AI: expecially by Sol 5.6 and Grok 4.7 for documentation.
L'obiettivo principale era quello di esercitarmi nell'usare AI agentiche per scrivere prodotti software in modo efficace.

Si tratta di un programma utilizzabile da terminale, che si collega all'audio del microfono e quello del dispositivo audio del pc tramite un proxy ( es. BlackHole per MacOS), producendo un elaborato testuale di quello che è stato registrato.  
L'intera pipeline di lavoro si svolge in locale, così come la conservazione di ogni elaborato, salvaguardando privacy e sicurezza.

# Installazione

Attualmente il programa e' in fase di pre-release ed è destinata a macOS Apple Silicon, in arrivo versioni compatibili anche per altri sistemi operativi...

### [Installazione per MacOS Apple Silicon](docs/INSTALLATION_MACOS.md)

### [Installazione per Windows](docs/INSTALLATION_WINDOWS.md)

### [Installazione per Linux]docs/INSTALLATION_LINUX.md)

# Funzionalità Principali

## Live Recording

Funzione principale del programma, nella quale l'utente può registrare l'audio dal proprio microfono e dall'audio del sistema contemporaneamente.

-------stcrivere come si usa il prodotto introducendo i comandi ed i loro flag via via

## Transcrizione di file

Possono essere trascritti anche file audio-video già esistenti nella propria macchina; in questo caso la trascrizione non avviene in streaming ed è quindi più veloce e meno esosa di risorse.
I formati di file supportati sono: mp3, m4a, mp4, aac.
Per quanto riguarda i formati wav, ad oggi non c'è ancora una funzione nativa che consenta una trascrizione che non simuli quella in velocità di streaming (verrà corretta con le successive implementazioni)

------fare come live recording

## Installazione

Servono macOS su Apple Silicon, Rust 1.88 o successivo e BlackHole. Installare e
configurare BlackHole seguendo la [guida macOS](docs/BLACKHOLE_MACOS.md), quindi
compilare:

```sh
git clone https://github.com/FabioTognaa/personal-transcriber.git
cd personal-transcriber
cargo build --release
```

ATTENZIONE: i modelli quantizzati sono molteplici e non sono inclusi nel repo di default. È possibile scaricarli tramite i comandi elencati qui sotto.

Dalla root del progetto

```sh
mkdir -p models
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin \
  -o models/ggml-small-q5_1.bin
```

### Scelta del modello

`--model-preset` seleziona un modello documentato senza scrivere percorsi. Il
valore predefinito è `small`; `large-v3-turbo` è il compromesso consigliato tra
accuratezza e velocità, `large-v3` la qualità massima. Un file GGML qualsiasi si
seleziona con `--model <percorso>`, che ha la precedenza sul preset.

Se avvii `start` da un terminale **senza** `--model` né `--model-preset`, compare
un menu che elenca i preset con lo stato `[scaricato]`/`[mancante]` e gli altri
file `.bin` presenti in `models/`:

```text
Seleziona il modello per la trascrizione:
  1) small            [scaricato] models/ggml-small-q5_1.bin
  2) medium           [mancante]  models/ggml-medium-q5_0.bin
  3) large-v3-turbo   [scaricato] models/ggml-large-v3-turbo-q8_0.bin
  4) large-v3         [mancante]  models/ggml-large-v3-q5_0.bin
Invio = default (small), numero = scegli, p = percorso personalizzato:
```

Invio accetta il default `small`, un numero sceglie la voce, `p` permette di
digitare un percorso. In script, CI o test (nessun terminale) il menu non compare
e vale il default `small`; passando `--model` o `--model-preset` il menu è saltato.

| Preset           | File                           | Dimensione | Uso consigliato            |
| ---------------- | ------------------------------ | ---------: | -------------------------- |
| `small`          | `ggml-small-q5_1.bin`          |    ~190 MB | predefinito, molto veloce  |
| `medium`         | `ggml-medium-q5_0.bin`         |    ~540 MB | via di mezzo               |
| `large-v3-turbo` | `ggml-large-v3-turbo-q8_0.bin` |    ~874 MB | migliore qualità/velocità  |
| `large-v3`       | `ggml-large-v3-q5_0.bin`       |    ~1,1 GB | qualità massima, più lento |

Per usare `large-v3-turbo`:

```sh
curl -fL \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin \
  -o models/ggml-large-v3-turbo-q8_0.bin
cargo run --release -- start --model-preset large-v3-turbo --language it
```

Su Apple Silicon con almeno 16 GiB, `large-v3-turbo` e `large-v3` restano sotto
il real-time factor 1 con margine ampio; le versioni F16 (`ggml-large-v3-turbo.bin`,
`ggml-large-v3.bin`) sono selezionabili con `--model` per la massima accuratezza.

### Prompt iniziale

Whisper accetta un prompt iniziale che orienta punteggiatura, maiuscole e
vocabolario senza alcuna post-elaborazione del testo. `start` usa
automaticamente un prompt predefinito per `--language` (`it` o `en`).
`--asr-prompt "testo"` lo sostituisce, ad esempio per aggiungere un glossario di
nomi, sigle e termini ricorrenti; `--asr-prompt ""` lo disattiva. Il prompt
effettivo è salvato in `session.json` e in ogni riga di `transcript.jsonl`.

## Cattura live

Verificare i nomi esatti dei dispositivi e i prerequisiti:

```sh
cargo run -- devices
cargo run -- doctor --probe-audio
```

Durante il probe di tre secondi occorre parlare e riprodurre audio verso BlackHole.
Il probe apre entrambi gli input, verifica permessi, segnale e integrità delle code,
ma non registra una sessione. Per selezionare dispositivi non predefiniti:

```sh
cargo run -- doctor --probe-audio \
  --microphone "Microfono MacBook Air" \
  --system-audio "BlackHole 2ch"
```

Avviare la cattura live omettendo `--input-wav`:

```sh
cargo run --release -- start \
  --microphone "Microfono MacBook Air" \
  --system-audio "BlackHole 2ch" \
  --language it
```

`--language` accetta `it` oppure `en` e usa `it` come valore predefinito.

Ogni sessione è una cartella `sessions/YYYY-MM-DD_HH-MM-SS/` (ora locale di
inizio). Il contenuto è descritto in [`sessions/README.md`](sessions/README.md).
La sessione conserva `audio/microphone.wav`, `audio/system.wav` e
`audio/mixed.wav`. I callback CoreAudio copiano soltanto i campioni in una coda
bounded: resampling, mix, disco, VAD e ASR avvengono fuori dal callback. Overflow,
disconnessione, cambio di sample rate o oltre cinque secondi di audio non abbinato
arrestano la sessione come fallita invece di perdere dati senza segnalarlo.
`status.json` espone picchi, presenza segnale, drift e blocchi callback persi.

## Simulatore WAV

`start` resta in foreground, normalizza il WAV in PCM mono 16 kHz e salva audio,
metadati e transcript in una nuova directory di sessione:

```sh
cargo run --release -- start \
  --input-wav benchmarks/fixtures/italian_synthetic.wav \
  --language it
```

Il simulatore carica il WAV in memoria ed è limitato esplicitamente a 64 MiB e
30 minuti. La cattura live non usa questo buffer completo e non ha tale limite.

Da un secondo terminale, usando la stessa `--sessions-dir` se diversa dal valore
predefinito `sessions`, si può controllare la sessione:

```sh
cargo run -- status
cargo run -- pause
cargo run -- resume
cargo run -- stop
```

`status` restituisce un report JSON con metadati, durata, stato, backlog, ritardo
rispetto all'ultimo segmento ASR e percorsi dei file della sessione.

La pausa riguarda solo la segmentazione e la trascrizione: l'audio continua a
essere scritto. Il segmento pendente viene chiuso quando inizia la pausa e
l'elaborazione riparte senza attraversare l'intervallo sospeso.

Il VAD usa chunk mono 16 kHz, soglia RMS, isteresi di avvio, preroll/postroll,
silenzio finale e durata massima. Da CLI si regolano solo soglia, innesco e fine
(`--vad-threshold`, `--vad-start-ms`, `--vad-end-ms`); preroll/postroll, durata
minima e massima restano fisse. Tutti i valori effettivi sono salvati in
`session.json`. Coda e metriche di segmentazione sono visibili in `status.json`.

Il worker ASR usa una coda bounded separata e scrive ogni segmento finale,
senza correzioni successive, in `transcript.jsonl`. Identità del modello,
configurazione d'inferenza, backlog e real-time factor sono persistiti nella
sessione.

## Trascrivere un file

Per trascrivere un file già esistente senza creare una sessione né salvare audio,
usa `transcribe`. Accetta WAV, MP3, M4A/MP4 e AAC:

```sh
cargo run --release -- transcribe riunione.mp3 --language it
```

L'input viene decodificato in streaming e normalizzato a PCM mono 16 kHz; MP3 e
MP4 sono letti con `symphonia` (puro Rust, nessun ffmpeg richiesto). Poiché non c'è
cattura in tempo reale, l'elaborazione procede più velocemente del tempo reale.
I WAV riusano il lettore in memoria, quindi ereditano il limite di 64 MiB / 30
minuti; MP3 e MP4 non hanno questo limite.
L'output sono due file accanto all'input:

- `riunione.txt`: testo puro, un segmento per riga;
- `riunione.md`: intestazione con il nome del file e righe con timestamp.

Se `riunione.txt` o `riunione.md` esistono già, il comando si ferma senza
sovrascriverli. Le opzioni di modello, lingua e segmentazione (`--model`,
`--model-preset`, `--language`, `--asr-prompt`, `--vad-threshold`,
`--vad-start-ms`, `--vad-end-ms`, `--asr-flash-attention`) sono le stesse di
`start`.

## Diagnostica ed export

`devices` elenca in JSON i dispositivi CoreAudio visibili, le direzioni supportate
e le configurazioni predefinite:

```sh
cargo run -- devices
```

`doctor` verifica Apple Silicon, directory delle sessioni, spazio libero, modello,
input audio e presenza di BlackHole senza modificare macOS:

```sh
cargo run -- doctor
cargo run -- doctor --model /percorso/al/modello.bin
```

L'assenza di un prerequisito produce `ready: false` ed exit code non zero. Senza
`--probe-audio`, permesso microfono e routing effettivo restano esplicitamente non
verificati e il report contiene un warning.

Ogni export deriva da `transcript.jsonl` e conserva il testo ASR senza correzioni:

```sh
cargo run -- export sessions/<sessione> --format text
cargo run -- export sessions/<sessione> --format markdown --output transcript.md
cargo run -- export sessions/<sessione> --format srt --output transcript.srt
cargo run -- export sessions/<sessione> --format vtt --output transcript.vtt
```

Sono supportati `jsonl`, `text`, `markdown`, `srt` e `vtt`. Senza `--format` viene
prodotto Markdown; senza `--output`, l'export viene scritto sullo standard output.
SIGINT e SIGTERM finalizzano i dati parziali, marcano la sessione come fallita e
ne riportano il percorso recuperabile. L'export rifiuta come destinazione qualsiasi
file canonico della sessione. Per esportare una sessione fallita o ancora attiva
occorre confermare esplicitamente con `--allow-partial`.

Il benchmark riproducibile, le fixture e i risultati di riferimento per Apple M5
sono descritti in [`benchmarks/README.md`](benchmarks/README.md).

## Limiti noti

- La v1 supporta macOS su Apple Silicon; Windows e Linux non sono supportati.
- La lingua deve essere scelta esplicitamente tra italiano e inglese; non esiste
  rilevamento automatico.
- Non sono incluse diarizzazione, identificazione degli speaker, UI o funzioni cloud.
- I file delle sessioni non sono cifrati a riposo.
- Il rilevamento del parlato usa una soglia energetica RMS, non un modello VAD.
- La cattura live non è ancora stata validata end-to-end su una call reale.
- `transcribe` elabora un file alla volta e in un solo thread: nel formato
  leggero non esistono pause, ripresa o stato di avanzamento.

## Documentazione

- [Funzionamento e architettura](docs/RELAZIONE.md)
- [Contesto completo e scelte architetturali](PROJECT_CONTEXT.md)
- [Roadmap di implementazione](ROADMAP.md)
- [Setup BlackHole su macOS](docs/BLACKHOLE_MACOS.md)
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
PERSONAL_TRANSCRIBER_MODEL=models/ggml-small-q5_1.bin \
  cargo test real_whisper_model_processes_local_audio -- --ignored
```

## Privacy e consenso

Prima di registrare o trascrivere una riunione, informa i partecipanti e verifica le
politiche aziendali e gli obblighi applicabili. Il fatto che l'elaborazione sia locale
non elimina questo requisito.

## Licenza

MIT. Vedi [LICENSE](LICENSE).
