# Piano della documentazione

Questo file non è documentazione del prodotto: è la mappa di come vogliamo
scrivere la documentazione. Serve a chi la scrive per non perdersi e per non
rimettere in discussione decisioni già prese. Quando il lavoro sarà finito,
questo file può essere cancellato.

## Il README della root

Il README deve raccontare due cose diverse e non mescolarle. La prima è **come
si usa** il programma quando è già pronto: comandi, in ordine, da copiare e
incollare. La seconda è **cosa fa** il programma: una descrizione delle
funzionalità per chi vuole capirle, non per chi le sta già usando.

L'ordine delle sezioni resta quello di oggi, con una sola aggiunta in mezzo.
Dopo l'installazione, e prima delle funzionalità, entra una nuova sezione
chiamata **Everyday use**. Tutto il resto resta dov'è.

### Everyday use

È la sezione del "sono pronto, come lo uso". Non spiega perché, non elenca
flag, non rimanda ad altre pagine: mostra soltanto i comandi nel loro ordine
reale. Va scritta immaginando qualcuno che ha appena finito l'installazione e
vuole la prima registrazione.

Ci sono due percorsi, uno dopo l'altro.

Il primo è la **registrazione dal vivo con due terminali**. Nel primo terminale
si lancia `start`. Nel secondo terminale si elencano *tutti* i comandi di
controllo che si possono lanciare contro quella sessione — `status`, `pause`,
`resume`, `stop` — ognuno con una riga che dice cosa fa. Nessuna tabella di
flag: quella vive altrove.

Il secondo è la **trascrizione di un file**: il comando `transcribe` con il
file e la lingua, e i due file che ne escono accanto all'input, il `.txt` e il
`.md`. Va detto chiaramente che in questo caso non nasce nessuna sessione.

### Le due funzionalità

Sotto Everyday use ci sono le due sezioni **Live Recording** e
**Trascrizione di file**. Qui l'obiettivo è capire la funzionalità, non
eseguirla: si descrive cosa accade **passo dopo passo**, in modo comprensibile,
denso e piuttosto astratto. Chi legge non deve annegare nelle tecnologie.

Le regole di queste due sezioni sono precise:

- niente numeri: nessuna soglia, nessuna dimensione, nessun tempo. I valori
  concreti non stanno qui;
- niente rimandi ad altre pagine o sezioni;
- niente nomi di funzioni, thread o file del codice;
- niente ridondanza: non si ripete quello che è già in Everyday use.

In pratica: concetti e passi, mai meccanica interna.

### Cosa resta com'è

Tutte le sezioni che oggi esistono nel README restano al loro posto:
`Cattura live`, `Simulatore WAV`, `Trascrivere un file`, `Diagnostica ed
export`, `Limiti noti`, `Documentazione`, `Sviluppo locale`, `Privacy e
consenso`, `Licenza`.

## Gli altri due file

La spiegazione **tecnica** — valori concreti, funzioni, thread, pipeline del
codice — vive in `docs/RELAZIONE.md`. Il README resta concettuale e quando ha
bisogno di un dettaglio tecnico lo lascia lì. Se durante il lavoro scopriamo
altro "funzionamento" da spiegare, finisce in RELAZIONE, non nel README.

`docs/COMMANDS_HELP.md` è il file dei **comandi e dei flag**, completo: ogni
comando e ogni opzione, senza starne fuori nessuno. È la fonte a cui si attinge
quando serve un flag, così il README può permettersi di non elencarli.

## Cosa non tocchiamo

Ci sono difetti noti nella struttura attuale della documentazione, e li
lasciamo stare: la "Installazione" che compare due volte, il link a
`ROADMAP.md` che non esiste, i due file di installazione per Windows e Linux
vuoti, e il link per Linux scritto male. Non sono parte di questo lavoro.
Al massimo, e solo se serve, si può sistemare la sintassi di quell'unico link
rotto.

## Cose da ricordare

Due fatti vanno tenuti presenti mentre si scrive, perché riguardano il prodotto
e non il testo.

La cattura dal vivo **non è mai stata provata end-to-end su una call reale**:
i test coprono la pipeline su un WAV, non l'ascolto vero dei
dispositivi. Va scritto con onestà.

Gli artefatti `live-transcript` che si trovano dentro `target/` provengono da
**un altro progetto**: non sono binari di questo repository e non vanno
documentati come tali.

Il README è in italiano. L'intro in inglese che c'è oggi resta com'è: la lingua
non è parte di questo lavoro.
