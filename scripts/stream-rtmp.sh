#!/bin/bash

# TO USE
#
#     ./shows/my_show/target/release/show_binary | ./stream-rtmp.sh audio
#
# This script will look for (and source) ./SECRETS and assume that
# that script has `define RTMP_INGEST` with an RTMP url+key that
# will be the sink of the stream.

audio=$1

. ./SECRETS

audio_args=''
if [[ "$audio" != '' ]]; then
    # HEY! This will do bad wrong things if your audio file names have spaces in them!
    audio_args="-stream_loop -1 -i $audio"
fi

# You can run a bandwidth test only by appending ?bandwidthtest=true
# to your RTMP_INGEST url. You can see the results of your tests at
# https://inspector.twitch.tv/
ffmpeg -re -probesize 1000 -analyzeduration 1000 -i pipe: $audio_args -vcodec copy -f flv "${RTMP_INGEST}"