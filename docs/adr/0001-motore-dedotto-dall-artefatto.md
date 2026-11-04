# Il motore è dedotto dall'artefatto

Scribroom ha due motori (whisper.cpp e ONNX Runtime) ma l'utente sceglie solo il
modello. La forma dell'artefatto decide il motore: un file `.bin` è Whisper, una
directory con i file ONNX è Parakeet. Così il menu resta unico e non esiste un
prodotto cartesiano motore × modello.

## Conseguenze

- Aggiungere un terzo motore richiede una nuova forma di artefatto o un
  discriminante esplicito, non un nuovo flag utente.
- Il motore viene registrato nel transcript (`engine`), perché la scelta non è
  più visibile dalla sola riga di comando.
