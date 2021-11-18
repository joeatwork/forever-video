#!/bin/bash

ffmpeg \
    -i $1 \
    -vf "format=yuv420p, scale=(iw*sar)*min(640/(iw*sar)\,480/ih):ih*min(640/(iw*sar)\,480/ih), pad=640:480:(640-iw*min(640/iw\,480/ih))/2:(480-ih*min(640/iw\,480/ih))/2" \
    -vcodec libx264 \
    -r ntsc \
    -acodec aac \
    -ar 44100 \
    -ac 2 \
    $2
