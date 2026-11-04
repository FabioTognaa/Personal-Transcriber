# Nessun modello predefinito

Non esiste un modello scelto automaticamente. `start`, `transcribe` e `doctor`
richiedono `--model`/`--model-preset`, oppure mostrano un menu con i soli modelli
presenti; senza terminale e senza scelta il comando fallisce. L'alternativa era un
default implicito (`small`), che rendeva invisibile una decisione di prodotto e
poteva far trascrivere con il modello sbagliato.

## Conseguenze

- Il primo avvio richiede un download esplicito; finché non esiste `models
  install`, l'errore rimanda alla guida.
- Il menu ordina per dimensione e non propone etichette "consigliato".
