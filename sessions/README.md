# Sessioni

In questa cartella finiscono tutti i dati di ogni registrazione vocale.

Ogni `start` crea una cartella, che viene nominata in base al giorno nel quale si avvia la registrazione, nel formato:

```text
YY-MM-DD
```

Se più sessioni partono nello stesso giorno, la seconda aggiunge un intero in fondo (`_2`, `_3`…).
L'ordinamento alfabetico coincide con quello cronologico per giorno; dentro lo stesso giorno dipende dal suffisso.
Questo nome comunque non funge da identificare. Puoi trovare l'UUID `current-session.json`.

Anche `transcribe` crea una cartella qui (stesso nome `YY-MM-DD`, `_2`, `_3`… su
collisione), ma non è una sessione live: contiene solo `session.json`,
`transcript.jsonl` e `<nome>.txt` con i timestamp, senza `audio/`, eventi,
`status.json` o `current-session.json`. Non viene scoperta da `status`, `pause`,
`resume` o `stop`, ma può essere letta da `export` come qualsiasi altra sessione
completata:

```sh
scrib export sessions/<cartella> --format text
```

Ogni registrazione e transcript resta sul disco locale e viene ignorata nel
repository.

In ogni cartella si possono trovare:

- ### /audio:
  cartella nella quale vengono salvati 3 file in formato .wav, che rappresentano la tracce audio di microfono, sistema dal quale blackhole riceve il segnale in output, ed una traccia con questi 2 canali mixati

- ### control.json:
  un file di log di tutti i comandi di controllo del trascriber che vengono passati una volta che il programma è in esecuzione

- ### session.json:
  file di metadati dell'intera sessione

- ### status.json:
  file di metadati relativo alla registrazione (chunking, lavoro del modello asr...) che si aggiorna a runtime

- ### transcript.jsonl:
  file nel quale vengono salvati i segmenti di testo ricostruiti dal motore ASR
  scelto (Whisper o Parakeet). Ogni riga registra anche il motore nel campo
  `engine` (`whisper` o `parakeet`), oltre a modello, SHA-256 e parametri di
  inferenza.


- ### .control.lock .runner.lock:
  sono token che attestano l'univocità della sessione

- current-session.json:
  file co le variabili globali della sessione: il percorso assoluto della cartella e l'UUID della sessione

## Layout di una sessione


`transcript.jsonl` è la fonte macchina: contiene metadati aggiuntivi oltre ai segmenti di codice.

## Trasformare transcript.jsonl in raw text 

```sh
cargo run -- export sessions/cartella_di_sessione --format markdown
cargo run -- export sessions/cartella_di_sessione --format text
```