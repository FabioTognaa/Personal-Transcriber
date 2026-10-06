# Fixture benchmark italiane

`italian_synthetic.wav` è generata localmente con la voce macOS `Alice` dal testo
`italian_synthetic.txt`. Serve come smoke test riproducibile, non come misura
realistica della qualità su riunioni.

`italian_public.wav` deriva da
[`It-il prezzo.ogg`](https://commons.wikimedia.org/wiki/File:It-il_prezzo.ogg),
registrata da Marta Carbone per Association Shtooka. La sorgente è distribuita con
licenza [CC BY 3.0 US](https://creativecommons.org/licenses/by/3.0/us/). Il file è
stato convertito in PCM mono 16 kHz senza modificare il parlato; il testo di
riferimento è in `italian_public.txt`.

Le fixture possono essere rigenerate su macOS con:

```sh
say -v Alice -f italian_synthetic.txt -o /tmp/italian_synthetic.aiff
ffmpeg -i /tmp/italian_synthetic.aiff -ac 1 -ar 16000 \
  -c:a pcm_s16le italian_synthetic.wav

curl -fL \
  'https://commons.wikimedia.org/wiki/Special:Redirect/file/It-il%20prezzo.ogg' \
  -o /tmp/italian_public.ogg
ffmpeg -i /tmp/italian_public.ogg -ac 1 -ar 16000 \
  -c:a pcm_s16le italian_public.wav
```

La voce sintetica può cambiare tra versioni di macOS; i report identificano sempre
l'audio effettivo tramite SHA-256.
