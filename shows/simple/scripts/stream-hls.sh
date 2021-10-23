#!/bin/sh

set -e

rm ./stream/* && true

./shows/simple/target/release/simple | \
    ffmpeg -f flv -re -i pipe: \
        -f hls -hls_time 6 ./stream/simple.m3u8
