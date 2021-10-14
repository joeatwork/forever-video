#!/bin/bash

. $(pwd)/SECRETS

./shows/simple/target/release/simple | \
    ffmpeg -loglevel debug -f h264 -i pipe: \
        -stream_loop -1 -i ./shows/simple/media/design-for-dreaming.mp3 \
        -f flv -vcodec libx264 -g 30 "$RTMP_INGEST"
