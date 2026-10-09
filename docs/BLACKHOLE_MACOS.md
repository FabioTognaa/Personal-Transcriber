# Setup BlackHole su macOS

BlackHole è un driver audio virtuale open source utilizzato dai dispositivi MacOS. Consente a un'applicazione di
produrre audio verso un dispositivo virtuale e a `personal-transcriber` di leggerlo come
input.

Il compito principale che ha è quello di instradare l'audio che si produce l'audio verso la pipeline di trascrizione del programma.

## Canali che BlackHole raggiunge

L'audio dei partecipanti deve raggiungere contemporaneamente:

1. cuffie o altoparlanti, così l'utente può ascoltare la riunione;
2. BlackHole, così il tool può registrarlo.

Il microfono è un input separato. Il tool miscela localmente microfono e BlackHole
per produrre la traccia di trascrizione v1.

## Configurazione prevista

1. Installare **BlackHole 2ch** seguendo le [istruzioni ufficiali](https://github.com/ExistentialAudio/BlackHole/wiki/Installation).
2. Aprire **Configurazione MIDI Audio** su macOS.
3. Creare un dispositivo **Multi-output**.
4. Selezionare BlackHole 2ch e l'uscita desiderata, ad esempio cuffie USB o
   Built-in Output.
5. Ogni volta che si entra in riunione o in chiamata, occorre che il dispositivo Multi-output sia impostato come output della call o come output di
   sistema prima di entrare nella riunione.
6. Lasciare il normale microfono selezionato come input della call.
7. In `personal-transcriber`, selezionare BlackHole come sorgente audio remoto e il
   microfono come sorgente locale.

## Verifica di configurazione consigliata

Prima di una riunione importante:

1. eseguire `personal-transcriber devices` sulla cli e verificare che BlackHole e microfono siano
   elencati;
2. iniziare a parlare e riprodurre un breve audio verso il dispositivo Multi-output;
3. eseguire `personal-transcriber doctor --probe-audio`; se necessario, passare i nomi
   esatti con `--microphone` e `--system-audio`;
4. verificare che `microphone_signal`, `system_signal` e `capture_integrity`
   risultino `pass`;
5. avviare una breve sessione e controllare - nella relativa sottocartella in /sessions - i file `audio/microphone.wav`,
   `audio/system.wav`, `audio/mixed.wav` e le metriche `capture` di `status`.

Il probe dura tre secondi per impostazione predefinita, non crea una sessione e non
modifica le impostazioni audio di macOS.

## Ripristino

Al termine, selezionare nuovamente cuffie/altoparlanti normali come output di sistema
o della piattaforma di call. Se l'audio sembra sparito, quasi sempre l'output è ancora
instradato solo a BlackHole.

## Limiti

Il dispositivo Multi-output può avere controlli volume differenti dal dispositivo
normale. BlackHole non risolve ritardi, eco, audio disattivato nella call o permessi
mancanti del microfono. `personal-transcriber` deve segnalare l'assenza di segnale, ma non
può garantire che l'utente abbia configurato correttamente ogni applicazione.
