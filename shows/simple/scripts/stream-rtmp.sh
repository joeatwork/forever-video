#!/bin/bash

. $(pwd)/SECRETS

./shows/simple/target/release/simple | ffmpeg -loglevel debug -f h264 -i pipe: -f flv -vcodec libx264 -g 30 "$RTMP_INGEST"