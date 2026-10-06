# Come funziona personal-transcriber

Questo documento spiega il programma a chi non lo ha mai visto: che problema risolve,
con quali pezzi è costruito, e cosa fa davvero dal momento in cui si lancia un
comando fino ai file lasciati sul disco.

È una descrizione del comportamento attuale, non una roadmap e non un manuale di
installazione. Per l'uso quotidiano resta il [README](../README.md). Per le decisioni
di prodotto resta [PROJECT_CONTEXT.md](../PROJECT_CONTEXT.md).

## Cosa fa, in una frase

`personal-transcriber` è un programma da terminale. Durante una riunione registra sul Mac
due audio — il microfono di chi parla e l'audio degli altri, letto tramite BlackHole —
li mescola, e scrive in italiano o inglese il testo di ciò che è stato detto. Tutto
resta sul computer. Nessun audio e nessun testo partono verso un servizio esterno.

Si può anche saltare la riunione e dare al programma un file WAV già pronto. In quel
caso la pipeline è la stessa: cambia solo la sorgente.

## Parole usate più avanti

- **Campione.** Un numero che descrive l'ampiezza dell'onda sonora in un istante.
- **PCM.** La sequenza di quei campioni. È l'audio "nudo", prima di MP3 o AAC.
- **Frequenza di campionamento.** Quanti campioni al secondo. Qui il formato interno
  è sempre **16.000 campioni al secondo, un solo canale** (mono). Whisper si aspetta
  proprio questo.
- **Chunk.** Un pezzetto di PCM lungo 20 millisecondi, cioè 320 campioni. Il programma
  non lavora sull'audio infinito: lo taglia in chunk e li passa avanti.
- **VAD.** Voice Activity Detection: decidere se un chunk è voce o silenzio, senza
  capire le parole.
- **Segmento.** Un intervallo di voce abbastanza lungo da meritare una trascrizione,
  con un inizio e una fine.
- **ASR.** Automatic Speech Recognition: il motore che trasforma i campioni in testo.
  Qui è Whisper, eseguito in locale da whisper.cpp.
- **BlackHole.** Un dispositivo audio virtuale. La call scrive l'audio anche lì, e
  questo programma lo legge come se fosse un microfono. Non entra in Zoom, Meet o
  Teams e non vede le loro credenziali.
- **JSONL.** Un file di testo con un oggetto JSON per riga. Si può aggiungere una riga
  senza riscrivere il file. È il formato canonico di eventi e trascrizione.
- **Sessione.** La directory di una registrazione, con audio, stato, eventi e testo.

## Stack

Ogni dipendenza ha un ruolo stretto. Non c'è un server, un database né un runtime
Python nel prodotto.

| Pezzo | Ruolo |
| --- | --- |
| Rust, edition 2024, minimo 1.88 | Linguaggio del programma. Un binario solo. |
| `clap` | Legge i comandi e le opzioni. |
| `cpal` | Elenca i dispositivi CoreAudio e apre i due stream di input. |
| `hound` | Legge e scrive WAV. |
| `crossbeam-channel` | Code a capacità fissa tra i thread. |
| `whisper-rs` 0.16.0 | Binding Rust verso whisper.cpp. Di default compila con Metal, quindi l'inferenza usa la GPU Apple. Core ML esiste come feature separata ed è spenta. |
| Modello `ggml-small-q5_1.bin` | Whisper small multilingue, quantizzato Q5_1, circa 190 MB. La v1 lo usa con lingua `it` o `en`. |
| `sha2` | Calcola il SHA-256 del modello e lo salva su ogni segmento, così si sa quale file ha prodotto il testo. |
| `serde` / `serde_json` | Legge e scrive JSON e JSONL. |
| `uuid` | Identifica sessione e segmento. |
| `signal-hook` | Intercetta Ctrl-C (`SIGINT`) e `SIGTERM` senza uccidere il processo a metà scrittura. |
| `libc` | Lock esclusivo su file (`flock`), usato per non avere due `start` insieme. |
| `tracing` | Log diagnostici su standard error. Il livello sale con `-v`. |
| `tempfile` | Solo nei test. |

Il testo finale è quello uscito da Whisper. Nessun secondo modello corregge, riassume
o parafrasa.

## I comandi

Tutti accettano `--sessions-dir` (default `sessions`) e `-v` ripetibile.

| Comando | Cosa fa | Cosa non fa |
| --- | --- | --- |
| `devices` | Elenca i dispositivi CoreAudio visibili, con formato predefinito e quali sono input/output di default. | Non apre uno stream. |
| `doctor` | Controlla piattaforma, directory, spazio disco, modello, input e BlackHole. | Senza `--probe-audio` non ascolta. Non cambia le impostazioni di macOS. |
| `doctor --probe-audio` | Apre microfono e BlackHole per qualche secondo (default 3) e verifica che arrivi segnale. | Non crea una sessione. |
| `start` | Resta in primo piano fino alla fine. Crea una sessione nuova e stampa il percorso. | Non sovrascrive una sessione già esistente. |
| `status`, `pause`, `resume`, `stop` | Si lanciano da un secondo terminale. Parlano con `start` tramite file, non tramite rete. | `pause` non ferma la registrazione. |
| `export` | Legge `transcript.jsonl` e produce JSONL, testo, Markdown, SRT o VTT. | Non modifica il testo. Rifiuta di scrivere sopra i file canonici della sessione. |

`start` senza `--input-wav` è la cattura live: microfono di default e, se non si passa
`--system-audio`, il primo input il cui nome contiene `blackhole`. I due dispositivi
devono essere diversi. Con `--input-wav` i dispositivi non servono: il WAV viene
riprodotto a velocità reale.

La lingua predefinita è `it`; `--language en` seleziona l'inglese. Non c'è
rilevamento automatico e la scelta vale per tutta la sessione.

## Il flusso, dall'inizio alla fine

```text
comando start
    → carica il modello e ne calcola l'identità
    → apre la sorgente (WAV oppure microfono + BlackHole)
    → crea la directory di sessione
    → tre lavori in parallelo:
         acquisizione → coda audio → loop principale
         loop principale → coda segmenti → VAD
         VAD → coda ASR → Whisper → transcript.jsonl
    → stop, fine del file, oppure segnale
    → chiude i WAV, scrive lo stato finale, stampa il percorso
```

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
8. Crea la directory `sessions/<millisecondi-unix>-<8 caratteri dell'uuid>/` e i file
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

### File WAV

Il file viene letto tutto, convertito e tenuto in memoria. Qualsiasi WAV intero a
8, 16, 24 o 32 bit, oppure float a 32 bit, viene portato a mono: se ci sono più
canali, si fa la media. Se la frequenza non è 16 kHz, un ricampionamento lineare
cambia il numero di campioni mantenendo la durata.

Poi i chunk escono **a velocità reale**. Un chunk che nel file inizia a 2,0 secondi
viene consegnato quando sono passati circa 2,0 secondi di orologio. Serve a provare
pause, code e ritardo come in una call, senza una call.

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
media dei quadrati dei campioni. Se l'RMS è almeno 0,02, il chunk è voce. I default,
tutti modificabili con le opzioni `--vad-*` e poi salvati in `session.json`, sono:

| Soglia | Default | Effetto |
| --- | --- | --- |
| Voce | RMS ≥ 0,02 | Distingue voce e silenzio. |
| Innesco | 60 ms di voce | Un click isolato non apre un segmento. |
| Fine | 800 ms di silenzio | Una pausa breve dentro la frase non la spezza. |
| Pre-roll | 200 ms | Quando la voce è confermata, si tiene anche un po' di audio precedente, per non tagliare l'attacco. |
| Post-roll | 200 ms | Si tiene un po' di audio dopo l'ultima voce, senza superare la fine del chunk corrente. |
| Minimo | 100 ms di voce | Sotto questa durata il segmento si scarta e il contatore `segments_discarded` sale. |
| Massimo | 30 s | Un intervento lungo viene tagliato e il contatore degli split sale. Il taglio non butta campioni: il seguito diventa il segmento successivo. |

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

## Da segmento a testo

Il worker ASR ha una coda da 8 segmenti. Qui l'invio è bloccante: se Whisper è
indietro, il segmentatore aspetta un posto in coda. Quell'attesa può riempire la
coda di segmentazione e far scattare il replay descritto sopra. L'acquisizione, nel
frattempo, continua.

Per ogni segmento nuovo Whisper gira con traduzione disattivata, lingua `it` o `en`
scelta all'avvio, temperatura 0, beam search di ampiezza 5 e al massimo 8 thread.
Flash attention è accesa. Il testo dei pezzi interni di Whisper viene concatenato
così com'è, senza ripulitura.

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
sessions/<id>/
  session.json        chi, quando, con quale configurazione
  status.json         fotografia aggiornata mentre gira, e stato finale
  events.jsonl        avvio, pause, riprese, stop, errori
  transcript.jsonl    solo segmenti finali, una riga JSON ciascuno
  control.json        ultimo comando richiesto dall'esterno
  audio/mixed.wav     ciò che è stato trascritto
  audio/microphone.wav   solo cattura live
  audio/system.wav       solo cattura live
  logs/               directory creata, ma i log di diagnostica vanno su stderr
```

`session.json` è la descrizione stabile: lingua, percorso del modello, identità
SHA-256, dispositivi o file di origine, soglie VAD, parametri ASR. `events.jsonl` è
il diario. `transcript.jsonl` è la fonte da cui derivano tutti gli export. Lo schema
attuale è la versione 3; un export di un'altra versione viene rifiutato.

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
modello leggibile, almeno un input, BlackHole visibile. Se il file si chiama
`ggml-small-q5_1.bin`, dimensione e SHA-256 devono essere quelli documentati. Un
altro nome di modello, se il file non è vuoto, passa. Sotto 1 GiB libero il disco è
un avviso; sotto 64 MiB è un fallimento. `ready: false` produce un exit code diverso
da zero.

Il permesso del microfono e il routing vero della call restano non verificati finché
non si usa `--probe-audio`, e anche quello dice solo se in quei secondi è arrivato
segnale. Non può sapere se Zoom sta mandando l'audio al dispositivo giusto.

## Cosa il programma rifiuta di fare

- Non manda audio, testo o telemetria fuori dal Mac.
- Non aggiorna sé stesso e non cancella sessioni da solo.
- Non cifra i file. La privacy dipende dai permessi della directory e da chi ha
  accesso al computer.
- Non separa le voci dei partecipanti.
- Non sceglie la lingua da solo e non gira su Windows o Linux.
- Non è un'interfaccia grafica e non è un bot della riunione.
- Non promette una trascrizione certificata parola per parola. "Raw" qui significa:
  l'audio originale resta la fonte, e il testo è esattamente l'uscita di Whisper,
  con accanto il modello che l'ha prodotto.

La prova su una call reale, con BlackHole installato, non fa ancora parte di ciò
che questo repository può garantire da solo. I controlli automatici coprono la
pipeline sul WAV e le regole di sessione; l'ascolto del mix vero resta una verifica
manuale.
