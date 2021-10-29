This is the source code behind whatever is happening at https://www.twitch.tv/joeatworld

# Building

You can build all of the artifacts in this repository with

```console
$ cargo build
```

The build depends on a local install of libx264 and librtmp.

The libx264 build, in particular, assumes version 155 of the library, and will fail to build on other versions.

The and the associated scripts (and general usefulness of the package ) require the ffmpeg tool set.

On WSL / Ubuntu I got these dependencies with:

```console
$ sudo apt install libx264-dev librtmp-dev ffmpeg
```

# Streaming a show

"Shows" are binaries that produce FLV-formatted media on their standard output. You can
see some examples in the `./shows` directory.

You can stream a show with the shell script in `/scripts/stream-rtmp.sh`.

Read the script for details of how to use it. (The usage is non-obvious, but the
script is very short.)
