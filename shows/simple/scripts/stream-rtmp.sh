#!/bin/bash

. $(pwd)/SECRETS

./shows/simple/target/release/simple | \
    ffmpeg -f flv -i pipe: \
        -stream_loop -1 -i ./shows/simple/media/design-for-dreaming.mp3 \
        -f flv -vcodec copy "$RTMP_INGEST"
