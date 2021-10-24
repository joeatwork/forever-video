#!/bin/sh

# Run with
#
#    ./target/release/my_show | stream-hls.sh
#
# HLS streaming files will appear in ./stream

set -e

rm ./stream/* && true

ffmpeg -f flv -re -i pipe: -f hls -hls_time 6 ./stream/stream.m3u8
