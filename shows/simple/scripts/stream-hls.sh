#!/bin/sh

set -e

rm ./stream/* && true

./shows/simple/target/release/simple | \
    ffmpeg -f h264 -i pipe: \
        -stream_loop -1 -i ./shows/simple/media/design-for-dreaming.mp3 \
        -f hls -hls_time 6 ./stream/simple.m3u8
