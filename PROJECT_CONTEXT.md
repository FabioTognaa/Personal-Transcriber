# personal-transcriber — contesto di progetto

## Stato e obiettivo

`personal-transcriber` è un tool CLI, inizialmente privato e open source in seguito, per
registrare e trascrivere localmente una riunione in corso su macOS Apple Silicon.

La versione 1 deve catturare:

- l'audio remoto riprodotto dal computer;
- il microfono dell'utente;
- una trascrizione italiana con latenza percepita di circa 3–15 secondi.

Il programma salva l'audio originale e la trascrizione in locale. Non invia audio,
testo o telemetria a servizi cloud. Il testo prodotto è l'output finale del motore
ASR senza LLM, riassunti, correzioni grammaticali o parafrasi.

Il prodotto non è una UI, un bot Zoom/Meet/Teams, un registratore forense, né un
servizio collaborativo. È un programma da terminale.

## Vincoli e decisioni confermate

- Prima piattaforma: macOS Apple Silicon.
- Piattaforme future: Windows e Linux, con un core il più possibile portabile.
- Interfaccia: CLI.
- Audio: una traccia finale mixata, composta da microfono locale e audio della call.
- Audio di sistema macOS: BlackHole, configurato dall'utente.
- Lingue supportate: italiano e inglese, selezionate esplicitamente per sessione.
- Lingua futura: rilevamento automatico.
- Elaborazione: solo locale.
- Latenza: segmenti finali dopo una pausa, circa 3–15 secondi.
- Audio originale: conservato localmente per sessione.
- Pause: continua a registrare l'audio, ma sospende la trascrizione.
- Cifratura a riposo: non inclusa nella v1.
- Licenza: MIT.
- Repository iniziale: privato su GitHub.
- Nome del progetto: `personal-transcriber`.

## Definizione precisa di “raw”

“Raw” non significa una trascrizione letterale certificata: un modello ASR può
fraintendere parole, ometterne alcune, applicare punteggiatura o normalizzare forme
linguistiche. In questo progetto significa:

1. l'audio originale è conservato come fonte primaria;
2. l'output ASR finale viene conservato esattamente come prodotto dal motore;
3. nessun secondo modello corregge, riassume, riformula o fonde segmenti;
4. ogni segmento conserva metadati sufficienti per risalire a modello e
   configurazione che lo hanno generato;
5. eventuali modifiche manuali future saranno dati separati dall'output ASR.

## Esperienza d'uso v1

Prima di una call l'utente configura macOS affinché l'audio in uscita venga inviato
sia alle cuffie/altoparlanti sia a BlackHole. Avvia poi il comando `start`, indicando
il microfono e BlackHole, se necessario. Il tool miscela le due sorgenti, salva
progressivamente l'audio e scrive a console e su disco i segmenti di trascrizione
finalizzati.

Durante una pausa esplicita l'acquisizione continua, ma gli intervalli audio non sono
inviati al motore ASR. Questo comportamento deve essere segnalato nel transcript con
un evento `transcription_paused`; non deve fingere che l'audio non esista.

Quando l'utente esegue `stop`, il tool termina i segmenti pendenti, rende coerenti i
metadati della sessione e indica i file prodotti. L'esportazione in formati leggibili
è un comando distinto.

## Comandi CLI previsti

```text
personal-transcriber devices
personal-transcriber doctor
personal-transcriber start [opzioni]
personal-transcriber status
personal-transcriber pause
personal-transcriber resume
personal-transcriber stop
personal-transcriber export <sessione> [opzioni]
```

`devices` elenca input e output audio rilevati. `doctor` verifica prerequisiti:
architettura, permessi microfono, modello locale, disponibilità BlackHole e spazio
su disco. Non deve provare ad acquisire audio nascosto o modificare la configurazione
di macOS.

`start` crea una directory di sessione e non sovrascrive una sessione esistente.
`status` visualizza durata, stato, ritardo della coda, dispositivi e percorso output.
`pause` e `resume` agiscono sulla trascrizione, non sulla registrazione.
`stop` è idempotente per quanto ragionevole: un secondo `stop` non deve corrompere
la sessione.

Gli export iniziali sono JSONL, testo semplice, Markdown e SRT/VTT. JSONL è il
formato canonico per gli eventi; gli altri sono derivati.

## Architettura

Il programma è una pipeline composta da confini espliciti:

```text
AudioSource (mic + BlackHole)
  → PCM normalizzato
  → mixer
  → writer audio
  → coda bounded
  → VAD / segmenter
  → ASR worker
  → eventi transcript append-only
  → console e exporter
```

Ogni componente deve dipendere da interfacce proprie, non da un dispositivo o file
specifico. Il core deve poter essere testato con una sorgente file che emette gli
stessi chunk PCM della cattura live. Questo consente test riproducibili e rende
l'ingresso live un adattatore anziché un refactoring del trascrittore.

### Cattura e normalizzazione

I callback audio devono fare il minimo indispensabile: assegnare un timestamp
monotono e inviare piccoli chunk PCM a una coda lock-free o a un canale bounded.
Non devono eseguire inferenza, accesso a disco o log sincroni.

Ogni traccia viene convertita in PCM mono 16 kHz prima del mix. Il mix deve evitare
clipping tramite normalizzazione/attenuazione deterministica. La sessione conserva,
quando possibile, le tracce sorgenti oltre al mix; la trascrizione v1 usa il mix.

Le sorgenti audio possono avere clock diversi. Il primo rilascio deve rilevare e
registrare eventuale drift, ma non promette una compensazione perfetta. Se il drift
risulta significativo in test di riunioni lunghe, il resampling adattivo sarà una
milestone dedicata.

### Segmentazione

Il VAD classifica finestre brevi come voce/non voce. Un segmento non termina al
primo istante di silenzio: è finalizzato solo dopo una soglia configurata, inizialmente
circa 800 ms. Devono esistere limiti massimi di durata per evitare segmenti
indefinitamente lunghi.

All'inizio si preferisce un VAD semplice e misurabile; il passaggio a WebRTC VAD è
giustificato da benchmark, non dall'astrazione.

La configurazione VAD effettiva viene registrata in `session.json`. `status.json`
espone profondità corrente e massima della coda di segmentazione, durata classificata
come voce o silenzio, segmenti finalizzati o scartati e split per durata massima.
L'audio originale viene scritto prima dell'invio al worker VAD. La coda resta
bounded e non applica una politica di scarto implicita: se si satura, la
segmentazione viene ricostruita dal WAV già persistito rispettando gli intervalli
di pausa. `status.json` rende osservabile questa modalità differita.

### Trascrizione

Il backend ASR previsto è `whisper.cpp`, richiamato da Rust attraverso
`whisper-rs` o binding strettamente controllati. La scelta è motivata da:

- esecuzione completamente locale;
- accelerazione Metal su Apple Silicon;
- API C/C++ adatta a un binario portabile;
- modelli quantizzati e distribuibili.

Whisper elabora finestre finite, non è un flusso infinito con stato perfetto. La v1
privilegia segmenti finali dopo una pausa anziché output provvisori instabili. Le
finestre possono sovrapporsi per non tagliare parole ai confini; la deduplicazione
deve essere esplicita, deterministica e coperta da test.

Il modello iniziale e la sua dimensione saranno scelti tramite benchmark sul Mac
target. Il criterio di accettazione è un real-time factor inferiore a 1 con margine,
non soltanto una buona qualità apparente.

Il modello è selezionabile per sessione tramite preset documentati (`small`,
`medium`, `large-v3-turbo`, `large-v3`) oppure con un percorso GGML esplicito. Ogni
preset è verificato per dimensione e SHA-256. `large-v3-turbo` è il modello
consigliato quando l'accuratezza conta più del margine RTF; su Apple Silicon M5
resta ben sotto RTF 1 (vedi `benchmarks/README.md`).

All'ASR viene fornito un prompt iniziale predefinito per lingua, sostituibile per
sessione con `--asr-prompt`. È un input del motore ASR, non una correzione
successiva: non viola la definizione di "raw" e non introduce un secondo modello. Il
prompt effettivo è registrato in `session.json` e in ogni record di
`transcript.jsonl`.

### Concorrenza e backpressure

Acquisizione, persistenza audio e inferenza hanno velocità differenti. Le code
devono essere bounded e la metrica della loro occupazione deve essere visibile in
`status`. Se l'inferenza rallenta, la registrazione audio resta prioritaria.

La politica v1 in caso di sovraccarico è: continuare a conservare l'audio, segnalare
ritardo crescente e completare la trascrizione in differita; non perdere
silenziosamente dati. Una politica di scarto può esistere solo come opzione esplicita
e mai come default.

### Persistenza

Una sessione è una directory indipendente, ad esempio:

```text
sessions/2026-10-07_01-17-03/
  session.json
  audio/
    mixed.wav
    microphone.wav
    system.wav
  events.jsonl
  transcript.jsonl
```

I nomi concreti possono evolvere, ma valgono i principi: scrittura incrementale,
timestamp monotoni relativi alla sessione, formato robusto a crash e nessuna
ricostruzione distruttiva dell'intero transcript.

`events.jsonl` registra ciclo di vita, pause, riprese, errori e stop. `transcript.jsonl`
registra segmenti finali. Ogni record transcript include almeno identificatore,
inizio/fine relativi all'audio, testo, lingua, modello e configurazione rilevante.

## BlackHole su macOS

BlackHole è un driver audio virtuale open source. Opera come dispositivo audio:
un'app può riprodurre verso di esso e un'altra può acquisirlo come input. Non intercetta
una piattaforma di videoconferenza, non richiede credenziali Zoom/Meet/Teams e non
invia dati fuori dal computer.

Il setup previsto è:

1. installare BlackHole;
2. in Configurazione MIDI Audio, creare un dispositivo multi-output con BlackHole e
   cuffie/altoparlanti;
3. impostarlo come output della call o del sistema;
4. selezionare BlackHole e il microfono come input nel tool;
5. usare cuffie per limitare eco e feedback.

La documentazione operativa deve spiegare come ripristinare l'output normale.
`doctor` deve riconoscere BlackHole se installato, ma non può garantire che il routing
di una call sia corretto: deve offrire un test audio esplicito.

## Stack

- Rust stabile, edition corrente supportata dal compilatore;
- `clap` per CLI e configurazione;
- `cpal` per enumerazione e cattura da dispositivi audio;
- canali Rust bounded e thread/worker dedicati;
- `whisper.cpp` tramite `whisper-rs` o binding equivalente;
- VAD semplice iniziale, WebRTC VAD dopo benchmark;
- `serde` per formati dati;
- JSONL per log/eventi e transcript iniziale;
- SQLite solo quando query, indicizzazione o metadati lo giustificheranno;
- GitHub Actions per qualità e build macOS.

Non usare Python come runtime del prodotto, server HTTP, database remoto, container
o dipendenze cloud nella v1. Possono essere usati come strumenti di sviluppo solo se
non diventano requisiti dell'utente finale.

## Qualità e test

I test devono includere:

- unit test per segmentazione, timestamp, mix e serializzazione;
- test d'integrazione che simulano stream da WAV a velocità reale;
- fixture audio corte e versionabili, prive di contenuti personali;
- test di interruzione e ripresa della sessione;
- test che verificano che `pause` conservi audio e sopprima gli eventi ASR;
- benchmark separati, mai confusi con i test funzionali.

Le metriche sono qualità della trascrizione su fixture rappresentative, real-time
factor, latenza end-to-end, occupazione massima delle code e assenza di perdita audio.

## Sicurezza, privacy e limiti

La mancanza di cifratura nella v1 è una decisione esplicita, non una garanzia di
privacy. Le sessioni devono stare in una directory prevedibile con permessi locali
ragionevoli. Il README deve ricordare che registrare e trascrivere una riunione
richiede informare i partecipanti e rispettare le politiche aziendali e le leggi
applicabili.

Il tool non deve caricare dati, inviare telemetria, eseguire update automatici o
eliminare audio senza un'azione esplicita dell'utente.

## Versioning e GitHub

Il progetto segue Semantic Versioning:

- `0.y.z` durante sviluppo pre-1.0;
- `MAJOR` per cambi incompatibili;
- `MINOR` per funzionalità compatibili;
- `PATCH` per correzioni compatibili.

I commit usano Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `build:`,
`ci:` e simili). `CHANGELOG.md` segue Keep a Changelog e registra modifiche utente
visibili, non ogni refactor interno.

Una release prevede: versione aggiornata, changelog aggiornato, test/CI verdi, tag
Git annotato e GitHub Release. Finché il repository è privato non cambia la disciplina
di versioning. La pubblicazione MIT è una scelta già fissata per quando sarà reso
pubblico.

## Milestone

1. Fondazioni Rust: CLI, configurazione, formati evento, logging e test.
2. Simulatore stream: WAV → PCM chunk → VAD → eventi, senza cattura live.
3. ASR offline e near-live su stream simulato, con benchmark Apple Silicon.
4. Session storage, pause/resume, stop robusto ed export.
5. Cattura macOS da microfono + BlackHole, mix e diagnostica.
6. Hardening: recovery, test end-to-end, CI, documentazione setup BlackHole.
7. Pubblicazione privata GitHub e prima prerelease.

Windows/Linux, rilevamento automatico lingua, speaker diarization e UI sono fuori
dalla v1. Ogni estensione deve essere progettata tramite una decisione architetturale
dedicata, senza alterare l'integrità della pipeline locale.

## Regole di implementazione

- Ogni modifica deve preservare l'elaborazione esclusivamente locale.
- L'acquisizione audio non può bloccare per l'inferenza.
- Nessun dato viene perso silenziosamente: errori e backlog sono eventi osservabili.
- Ogni comportamento non ovvio va documentato vicino al codice e nel README.
- Le dipendenze audio e i modelli devono essere versionati o fissati esplicitamente.
- Ogni feature osservabile deve includere test proporzionati al rischio.
- Prima di cambiare formati di sessione o semantica dei comandi, aggiornare questo
  documento, il changelog se necessario e le fixture/test.
