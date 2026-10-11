# Come funziona personal-transcriber

Questo documento spiega il programma a chi non lo ha mai visto: che problema risolve,
con quali pezzi è costruito, e cosa fa davvero dal momento in cui si lancia un
comando fino ai file lasciati sul disco.

È una descrizione del comportamento attuale, non una roadmap e non un manuale di
installazione. Per l'uso quotidiano resta il [README](../README.md). Per le decisioni
di prodotto resta [PROJECT_CONTEXT.md](../PROJECT_CONTEXT.md).

## Cosa fa

`personal-transcriber` è un programma da terminale. Durante una riunione registra sul Mac
due audio — il microfono di chi parla e l'audio degli altri, letto tramite BlackHole —
li mescola, e scrive in italiano o inglese il testo di ciò che è stato detto.
Tutto questo viene fatto in locale al 100%.
Nessun audio e nessun testo partono verso un servizio esterno e viene garantita la privacy dei file prodotti.

Il trascrittore può essere usato anche per file audio/video già pronti, in quel caso vngono parsati e viene prodotta la trascrizione audio di tutto quello che viene detto

## Parole usate più avanti

- **Campione.**
  Un numero che descrive l'ampiezza dell'onda sonora in un istante.
- **PCM.**
  La sequenza di quei campioni senza alcuna compressione. È l'audio "nudo", prima di MP3 o AAC.
- **Frequenza di campionamento.**
  Quanti campioni al secondo. Qui il formato interno
  è sempre **16.000 campioni al secondo, un solo canale** (mono). Whisper si aspetta
  proprio questo.
- **Chunk.**
  Un pezzetto di PCM lungo 20 millisecondi, cioè 320 campioni. Il programma, in questo modo, può gestire l'audio in modo efficiente e senza bloccarsi.
- **VAD.**
  Voice Activity Detection: un metodo per riconoscere se il chunk contiene voce o silenzio
- **Segmento.**
  Un intervallo di voce abbastanza lungo da meritare una trascrizione,
  con un inizio e una fine. Vengono considerati segmento di voce un insieme di chunk.
- **ASR.**
  Automatic Speech Recognition: il motore che trasforma i campioni in testo.
  In questo caso viene usato Whisper, un LLM adibito proprio a fare speech recognition e a tradurre lo spettrogramma del parlato.
- **JSONL.**
  Formato JSON dei file di log e di trascrizione. Viene salvato per affiancare ai segmenti di prlato prodotti dei metadati.

- **Sessione.**
  La directory prodotta da una registrazione, con audio, stato, eventi e testo.

## Stack

- **Rust v1.88+**
  Linguaggio principale del programma.
- **Clap**
  Legge ed esegue il parsing delle righe di comando. Setta le opzioni e i valori di default.
- **`cpal`**
  Serve per aprire i due stream di input e catturare i campioni di audio analogici
- **`hound`**
  Serve per leggere e scrivere file WAV (PCM), in streaming, senza decodifica di altri formati.
- **`whisper-rs`**
  È il motore che serve per far girare i modelli AI Whisper, una volta che le rispettive matrici dei pesi vengono caricate in memoria. permette la trascrizione vera e propria del parlato

### File in Input

Attualmente i file supportati sono `wav`, `mp3`, `m4a`, `mp4` e `aac`. Un'estensione diversa viene rifiutata prima di decodificare.
Un WAV viene letto tutto e tenuto in memoria, quindi ha un tetto: oltre 64 MiB o oltre 30 minuti viene rifiutato. I formati compressi vengono invece decodificati un pacchetto alla volta, con memoria costante e senza limite di durata.
In questa fase occorre portare il file a PCM mono a 16 kHz, ma il file non viene convertito in Wav: viene trattato direttamente nel suo formato, per evitare lo spreco di computazione di un Wav che verrebbe scartato alla fine.
Se la frequenza non è 16 kHz, un ricampionamento lineare cambia il numero di campioni mantenendo la durata.

La segmentazione qui non avviene in tempo reale. Attualmente, per un file mp4 di 90m, vengono impiegati all'incirca 30m di trascrizione.
Ogni chunk viene scritto e processato da Whisper alla massima velocità.

Una volta conclusa la trascrizione vengono scritti due file accanto all'input, con lo stesso nome e le estensioni `.txt` e `.md`. I file esistenti non vengono sovrascritti. Non nasce nessuna sessione e nessun audio viene conservato: `transcribe` non registra e non produce un transcript canonico JSONL esportabile.
[DA MIGLIORARE]

## Il flusso, dall'inizio alla fine

`start` fa questo, in ordine.

1. Installa i gestori di `SIGINT` e `SIGTERM`. Da quel momento un Ctrl-C è un flag
   letto dal loop, non un'uscita immediata.
2. Verifica lingua, parametri di inferenza e che il file del modello esista.
3. Calcola il SHA-256 del modello e carica Whisper in memoria, con GPU accesa.
4. Prende un lock esclusivo su `sessions/.runner.lock`. Un secondo `start` nella
   stessa directory fallisce finché il primo è vivo. Se il processo precedente è
   morto, il lock del sistema operativo si libera da solo.
5. Se `current-session.json` punta a una sessione ancora `starting`, `running`,
   `transcription_paused` o `stopping`, la marca `failed` e aggiunge un evento di
   errore. È il recupero dopo un crash: non si finge che quella sessione sia ancora
   in corso.
6. Apre la sorgente. Per un WAV controlla anche i tetti del simulatore: file oltre
   64 MiB, oppure durata oltre 30 minuti, vengono rifiutati prima di partire. La
   cattura live non ha questi tetti, perché non tiene l'intero audio in RAM.
7. Controlla lo spazio disco. Per un WAV stima i byte della sessione. Per la live
   pretende almeno 64 MiB liberi, e ricontrolla ogni 5 secondi.
8. Crea la directory `sessions/YYYY-MM-DD_HH-MM-SS/` (ora locale di inizio) e i file
   vuoti. Lo stato passa a `running` solo dopo che il worker ASR è partito. Se
   quell'avvio fallisce, la sessione viene chiusa come `failed` con i WAV finalizzati,
   anche se sono ancora a zero campioni.
9. Parte il loop. Quando la sorgente finisce, o arriva `stop`, o arriva un segnale,
   svuota ciò che è già in coda, chiude il segmento di voce aperto, aspetta Whisper
   e scrive lo stato finale.

`start` stampa solo il percorso della directory. `status` stampa il rapporto JSON.

## Le due sorgenti

Il resto del programma non sa se i chunk arrivano da un file o da CoreAudio. Vede
solo PCM mono a 16 kHz, con un timestamp in microsecondi dall'inizio della sessione.

### Microfono e BlackHole

Si aprono due stream `cpal` separati, ciascuno nel formato nativo del dispositivo
(`f32`, `i16` o `u16`). Il callback di CoreAudio fa solo tre cose: converte i
campioni in `f32`, segna l'istante rispetto a un orologio comune, e tenta di metterli
in una coda da 512 blocchi. Non scrive su disco, non segmenta, non chiama Whisper.

Se la coda è piena, il callback incrementa un contatore e torna subito. Appena il
loop vede un blocco perso, **ferma la sessione con un errore**. Non continua
scartando audio in silenzio. Lo stesso vale se uno stream segnala un errore, se i
due callback si disconnettono, o se a metà strada cambia la frequenza di
campionamento.

Un thread dedicato, fuori dal callback, normalizza e mescola.

## Normalizzazione e mix

Ogni sorgente live passa da un ricampionatore incrementale verso 16 kHz mono. I due
risultati entrano in code separate.

All'inizio i dispositivi non partono nello stesso microsecondo. Il mixer misura lo
scarto e antepone silenzio alla sorgente partita dopo, così i primi chunk descrivono
lo stesso intervallo di tempo. Se le due partenze distano più di cinque secondi, è
un errore.

Da lì in poi emette chunk da 20 ms solo quando **entrambe** le code hanno abbastanza
campioni. Il mix di ogni coppia è `(microfono + sistema) × 0,5`, poi limitato
all'intervallo da −1 a 1. L'attenuazione è fissa: evita il clipping quando i due
segnali sono entrambi forti, ed è ripetibile.

Il programma conta anche quanti campioni sono arrivati da ciascuna parte. La
differenza, convertita in microsecondi, è `clock_drift_us` dentro `status.json`.
Viene registrata. Non viene corretta: se in una call lunga i due orologi divergono
davvero, il riallineamento adattivo non è in questa versione.

Se una sorgente continua e l'altra no, il buffer della prima non cresce oltre
cinque secondi. Superata quella soglia la sessione fallisce con un errore esplicito
di stallo.

In chiusura, l'ultimo pezzo può essere più corto di 20 ms. La traccia più corta
viene completata con zeri, così non si butta la coda di quella più lunga.

Ogni chunk live produce tre WAV: `microphone.wav`, `system.wav` e `mixed.wav`.
La trascrizione usa solo il mix. Con il simulatore esiste soltanto `mixed.wav`.

Un campione è considerato "segnale presente" se il picco assoluto raggiunge 0,001.
`doctor --probe-audio` usa questa soglia, e fallisce anche se durante la prova è
stato perso almeno un blocco di callback. Un drift oltre 20 ms in quella prova
breve è un avviso, non un fallimento.

## Scrivere l'audio prima di capirlo

Il loop principale, ogni 5 ms circa, fa nell'ordine:

1. Legge un eventuale comando `pause`, `resume` o `stop`.
2. Legge un eventuale segnale di terminazione.
3. Prende il prossimo chunk dalla coda audio.
4. Lo scrive sul WAV e aggiorna l'header. Dopo ogni chunk il file è già un WAV
   leggibile, non solo alla fine.
5. Se la trascrizione è attiva, passa il chunk al segmentatore.
6. Circa quattro volte al secondo riscrive `status.json`.

L'audio quindi tocca il disco prima di essere interpretato. Se più avanti Whisper è
lento, la registrazione non aspetta l'inferenza per esistere.

Lo stato `status.json` include la posizione audio, i chunk scritti, la profondità
attuale e massima di ogni coda, le durate classificate come voce o silenzio, i
segmenti chiusi o scartati, e — solo in live — picco, segnale, drift e blocchi
persi.

## Come nasce un segmento

Il VAD non è un modello neurale. Per ogni chunk calcola l'RMS, cioè la radice della
media dei quadrati dei campioni. Se l'RMS è almeno 0,02, il chunk è voce. I default
sono elencati sotto e vengono tutti salvati in `session.json`. Da CLI si regolano
solo voce, innesco e fine (`--vad-threshold`, `--vad-start-ms`, `--vad-end-ms`);
pre-roll, post-roll, minimo e massimo restano fissi:

| Soglia    | Default            | Effetto                                                                                                                                      |
| --------- | ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Voce      | RMS ≥ 0,02         | Distingue voce e silenzio.                                                                                                                   |
| Innesco   | 60 ms di voce      | Un click isolato non apre un segmento.                                                                                                       |
| Fine      | 800 ms di silenzio | Una pausa breve dentro la frase non la spezza.                                                                                               |
| Pre-roll  | 200 ms             | Quando la voce è confermata, si tiene anche un po' di audio precedente, per non tagliare l'attacco.                                          |
| Post-roll | 200 ms             | Si tiene un po' di audio dopo l'ultima voce, senza superare la fine del chunk corrente.                                                      |
| Minimo    | 100 ms di voce     | Sotto questa durata il segmento si scarta e il contatore `segments_discarded` sale.                                                          |
| Massimo   | 30 s               | Un intervento lungo viene tagliato e il contatore degli split sale. Il taglio non butta campioni: il seguito diventa il segmento successivo. |

Il segmentatore gira su un thread proprio. Il loop principale gli manda i chunk con
un invio non bloccante, su una coda da 64. Se la coda è piena, l'audio è già sul
WAV. Il loop alza `segmentation_replay_required`: da quel momento non manda altri
chunk al segmentatore vivo. Alla chiusura rilegge `mixed.wav` e rifà la
segmentazione solo sugli intervalli in cui la trascrizione era attiva. Gli
intervalli di pausa restano fuori.

Whisper riceve il segmento solo quando è chiuso. Non ci sono bozze che cambiano
mentre la persona sta ancora parlando. La latenza percepita è quindi "quanto dura
la frase, più l'inferenza", non un flusso di parole aggiornato ogni decimo di
secondo. L'obiettivo di progetto è un segmento finale nell'ordine dei 3–15 secondi,
non un sottotitolo istantaneo.

## Whisper

Il progetto si appoggia a `whisper-rs` per usare whisper.cpp in locale. Fa girare uno dei modelli che si trovano in `models/`, caricati in memoria una volta sola quando il programma viene inizializzato. Il suo compito è tradurre in testo i campioni che il VAD gli passa come PCM float mono a 16 kHz.

Il motore non decide dove inizia e finisce il parlato e non decide i tempi: riceve segmenti già ritagliati e restituisce solo il testo. Il comportamento si legge seguendo quello che accade dal caricamento fino alla stringa finale.

1. Viene validata la config di inferenza: numero di thread, strategia di decodifica, ampiezza del fascio, temperatura e prompt devono essere coerenti fra loro; Core ML viene rifiutato se il binario non è stato compilato per supportarlo.
2. Il file GGML viene aperto e i pesi caricati in memoria, con la GPU accesa (Metal su Apple Silicon) e la flash attention attiva salvo disattivazione esplicita. Il contesto resta vivo per tutta la sessione e viene riusato da ogni segmento.
3. Prima del caricamento viene calcolato lo SHA-256 del file del modello. Il calcolo ha una cache: finché dimensione e data del file non cambiano, l'hash viene riletto senza rileggere i pesi. L'hash finisce in ogni riga del transcript.
4. Per ogni segmento viene creato uno stato di decodifica nuovo. I pesi sono condivisi, lo stato no: è questo che rende economico trascrivere molti segmenti brevi uno dopo l'altro.
5. La chiamata vera e propria esegue la pipeline classica di Whisper, in quattro fasi:
   - **spettrogramma log-Mel**: i campioni diventano uno spettrogramma a 80 canali, riempito fino alla finestra di 30 secondi che Whisper usa come unità;
   - **encoder**: un transformer traduce lo spettrogramma in feature acustiche;
   - **decoder**: genera i token del testo uno alla volta, con la strategia scelta (beam search o greedy), fermandosi al token di fine;
   - **segmenti e timestamp**: Whisper produce micro-segmenti con i propri timestamp.
6. I timestamp interni di Whisper vengono ignorati. Il programma concatena il testo di tutti i micro-segmenti in un'unica stringa e associa a quella stringa l'intervallo deciso dal VAD, non quello di Whisper. La granularità temporale del transcript è quindi quella dei segmenti, non delle singole parole.

## Da segmento a testo

Il worker ASR ha una coda da 8 segmenti. Qui l'invio è bloccante: se Whisper è
indietro, il segmentatore aspetta un posto in coda. Quell'attesa può riempire la
coda di segmentazione e far scattare il replay descritto sopra. L'acquisizione, nel
frattempo, continua.

Per ogni segmento nuovo Whisper gira con traduzione disattivata, lingua `it` o `en`
scelta all'avvio, temperatura 0, beam search di ampiezza 5 e al massimo 8 thread.
Flash attention è accesa. Se è impostato un prompt iniziale, viene anteposto al
decoder: orienta punteggiatura, maiuscole e vocabolario, ma il testo resta l'uscita
del motore ASR, senza correzioni successive. Il testo dei pezzi interni di Whisper
viene concatenato così com'è, senza ripulitura.

Il risultato è una riga di `transcript.jsonl`: identificatore, inizio, fine, testo,
lingua, nome del modello, SHA-256 del file modello, e i parametri di inferenza
effettivi. Due segmenti con lo stesso inizio e la stessa fine non vengono scritti
due volte. Serve perché il replay può riproporre un intervallo già trascritto. Il
confronto è sugli estremi temporali, non sul testo.

Le metriche ASR in `status.json` contano segmenti trascritti, durata audio, durata
di inferenza e un real-time factor in millesimi: millisecondi di inferenza ogni
secondo di audio. Sotto 1000 l'inferenza è più veloce del tempo reale. `status`
mostra anche il ritardo fra la posizione audio e la fine dell'ultimo segmento
trascritto, tranne durante la pausa.

## Pause, stop, segnali, crash

I comandi di controllo non sono messaggi in memoria. Il secondo processo prende un
altro lock, `.control.lock`, scrive `control.json` con un numero di generazione che
cresce, e aspetta fino a 3 secondi che `status.json` riporti quella generazione.

Il loop di `start` applica il comando solo se lo stato lo consente.

- `pause` da `running` porta a `transcription_paused`, scrive l'evento e chiude
  subito il segmento aperto. L'audio dopo quel punto continua a essere registrato,
  ma non entra nel VAD e non entrerà nemmeno in un replay.
- `resume` fa l'inverso. Il buco di pausa non viene attraversato da un segmento.
- `stop` porta a `stopping`. La sorgente smette di produrre. I chunk già in coda
  vengono ancora scritti e, se si era in `running`, anche segmentati. Poi si chiude.
- Un secondo `stop` su una sessione già `completed` non rifà il lavoro: restituisce
  lo stato com'è. Su una sessione `failed` i comandi di controllo sono rifiutati.

`SIGINT` e `SIGTERM` seguono la stessa chiusura, ma lo stato finale è `failed`, con
l'evento "termination signal received; partial session finalized". I WAV vengono
comunque finalizzati. Il processo esce con errore e il percorso della sessione
parziale resta recuperabile.

Se il processo muore senza passare da questa chiusura, il lock di esecuzione cade
con lui. Il `start` successivo marca quella sessione `failed` con l'evento
"previous process ended without cleanup". I chunk il cui header WAV era già stato
aggiornato restano leggibili. Un chunk interrotto a metà scrittura può mancare.

Una sola sessione alla volta può essere "quella corrente": `current-session.json`
punta sempre a un percorso assoluto contenuto in `--sessions-dir`. Un percorso che
esce da quella directory viene rifiutato.

## I file di una sessione

```text
sessions/YYYY-MM-DD_HH-MM-SS/
  session.json        chi, quando, con quale configurazione
  status.json         fotografia aggiornata mentre gira, e stato finale
  events.jsonl        avvio, pause, riprese, stop, errori
  transcript.jsonl    solo segmenti finali, una riga JSON ciascuno
  control.json        ultimo comando richiesto dall'esterno
  audio/mixed.wav     ciò che è stato trascritto
  audio/microphone.wav   solo cattura live
  audio/system.wav       solo cattura live
```

`session.json` è la descrizione stabile: lingua, percorso del modello, identità
SHA-256, dispositivi o file di origine, soglie VAD, parametri ASR e prompt
iniziale. `events.jsonl` è il diario. `transcript.jsonl` è la fonte da cui derivano
tutti gli export. Lo schema attuale è la versione 3; un export di un'altra versione
viene rifiutato. Il campo `prompt` è stato aggiunto dopo la versione 3 come campo
opzionale, quindi i transcript precedenti restano leggibili.

Gli stati possibili sono `starting`, `running`, `transcription_paused`, `stopping`,
`completed`, `failed`.

## Export e diagnostica

`export` rilegge il JSONL, controlla che ogni riga appartenga alla sessione, che i
tempi siano ordinati e che la fine non preceda l'inizio, poi rende lo stesso testo:

- `text`: le frasi una dopo l'altra;
- `markdown`: le frasi con i timestamp;
- `srt` e `vtt`: sottotitoli temporizzati;
- `jsonl`: le stesse righe canoniche.

Senza `--output` il risultato va sullo standard output. Con `--output`, la scrittura
è atomica: prima un file temporaneo, poi la rinomina. Non può puntare a
`session.json`, `events.jsonl`, `transcript.jsonl`, `status.json`, `control.json` né
ai WAV.

`doctor` senza probe fallisce se manca una di queste condizioni: macOS su Apple
Silicon (`aarch64`), directory di sessione utilizzabile, almeno 64 MiB liberi,
modello leggibile, almeno un input, BlackHole visibile. Se il file corrisponde a un
modello documentato (`ggml-small-q5_1.bin`, `ggml-medium-q5_0.bin`,
`ggml-large-v3-turbo-q8_0.bin`, `ggml-large-v3-q5_0.bin` e le loro varianti in
`src/model.rs`), dimensione e SHA-256 devono essere quelli attesi. Un altro nome di
modello, se il file non è vuoto, passa con un avviso nel dettaglio. Sotto 1 GiB
libero il disco è un avviso; sotto 64 MiB è un fallimento. `ready: false` produce un
exit code diverso da zero.

Il permesso del microfono e il routing vero della call restano non verificati finché
non si usa `--probe-audio`, e anche quello dice solo se in quei secondi è arrivato
segnale. Non può sapere se Zoom sta mandando l'audio al dispositivo giusto.

La prova su una call reale, con BlackHole installato, non fa ancora parte di ciò
che questo repository può garantire da solo. I controlli automatici coprono la
pipeline sul WAV e le regole di sessione; l'ascolto del mix vero resta una verifica
manuale.
