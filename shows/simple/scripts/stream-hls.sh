#!/bin/sh

set -e

rm ./stream/*

./shows/simple/target/release/simple | \
    ffmpeg -f h264 -i pipe: -f hls -hls_time 6 ./stream/simple.m3u8
