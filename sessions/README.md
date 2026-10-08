# Sessioni

In questa cartella finiscono tutti i dati di ogni registrazione vocale.

Ogni `start` crea una cartella, che viene nominata in base al momento nel quale si avvia la registrazione, nel formato:

```text
YYYY-MM-DD_HH-MM-SS
```

L'ordinamento alfabetico coincide con quello cronologico. 
Se due sessioni partono nello stesso secondo, la seconda assume lo stesso nome aggiungendo un intero in fondo per discriminare il nome. 
Questo nome comunque non funge da identificare. Puoi trovare l'UUID `current-session.json`.

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
  file nel quale vengono salvati i segmenti di testo ricostruiti da Whisper


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