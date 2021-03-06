// This file is heavily sourced from libavformat/rtmpproto.c

//   https://github.com/FFmpeg/FFmpeg/blob/05f9b3a0a570fcacbd38570f0860afdabc80a791/libavformat/rtmppkt.c
//
// As such, it is licensed under version 2.1 of the GNU Lesser General Public License,
// or (at your option) any later version.

#[macro_use]
extern crate maplit;

mod mixer;

use std::convert::TryFrom;
use std::error::Error;
use std::fmt::Display;
use std::time::Instant;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use rml_amf0::Amf0Value;
use rml_rtmp::chunk_io::{ChunkDeserializer, ChunkSerializer};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::messages::{PeerBandwidthLimitType, RtmpMessage, UserControlEventType};
use rml_rtmp::time::RtmpTimestamp;

use mixer::Mixer;

struct Clock(Instant);

impl Clock {
    fn timestamp(&self) -> RtmpTimestamp {
        let millis = self.0.elapsed().as_millis();
        let millis32 = millis as u32;
        RtmpTimestamp { value: millis32 }
    }
}

const READ_BUFFER_SIZE: usize = 4096;
const PRE_MIXER_CHANNEL_BUFFER_SIZE: usize = 100;
const MAX_CLIENT_COUNT: usize = 10;

#[derive(Debug)]
struct WriteMessage {
    message: RtmpMessage,
    force_uncompressed: bool,
    can_be_dropped: bool,
}

#[derive(Debug)]
struct ClientError {
    message: String,
}

impl<T: Display> From<T> for ClientError {
    fn from(other: T) -> Self {
        ClientError {
            message: format!("{}", other),
        }
    }
}

struct ClientStream {
    client: TcpStream,
    clock: Clock,
    serializer: ChunkSerializer,
    deserializer: ChunkDeserializer,
    buf: [u8; READ_BUFFER_SIZE],
    next_stream_id: f64,
    bytes_since_ack: u32,
    ack_after_bytes: u32,
}

impl ClientStream {
    async fn connect_to_client(mut client: TcpStream) -> Result<ClientStream, ClientError> {
        let mut buf = [0u8; READ_BUFFER_SIZE];

        let mut handshake = Handshake::new(PeerType::Server);
        let hs_start = handshake.generate_outbound_p0_and_p1().unwrap();

        client.write_all(&hs_start).await?;

        let first_input = loop {
            let n = client.read(&mut buf).await?;
            if n == 0 {
                return Err("client went away during handshake".into());
            }

            let shake_progress = handshake.process_bytes(&buf[..n])?;
            match shake_progress {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    client.write_all(&response_bytes).await?;
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    client.write_all(&response_bytes).await?;
                    break remaining_bytes;
                }
            }
        };

        let mut deserializer = ChunkDeserializer::new();
        let mut serializer = ChunkSerializer::new();

        // Scan forward through everything the client says until it asks to connect.
        // This could break in a bunch of ways (the client could, for example, ask for
        // connection settings we ignore.)
        let mut input = &first_input[..];
        let connect_trans_id = loop {
            if let Some(payload) = deserializer.get_next_message(input)? {
                match payload.to_rtmp_message()? {
                    RtmpMessage::Amf0Command {
                        command_name,
                        transaction_id,
                        ..
                    } if command_name == "connect" => {
                        break transaction_id;
                    }
                    other => {
                        eprintln!("skipping client rtmp message before connect {:?}", other)
                    }
                };
            }

            let n = client.read(&mut buf).await?;
            if n == 0 {
                return Err("client went away before trying to connect".into());
            }
            input = &buf[..n];
        };

        let packet = serializer
            .set_max_chunk_size(128, RtmpTimestamp::new(0))
            .unwrap();
        client.write_all(&packet.bytes).await?;

        // We really oughta wait to read chunk size here.
        let mut stream = Self {
            client,
            clock: Clock(Instant::now()),
            serializer,
            deserializer,
            buf,
            next_stream_id: 3.0,
            bytes_since_ack: 0,
            ack_after_bytes: 1048576,
        };

        let msg0 = stream.read_message().await?;
        if let Some((_, RtmpMessage::SetChunkSize { size })) = msg0 {
            stream
                .deserializer
                .set_max_chunk_size(usize::try_from(size).unwrap())?;
        } else {
            // TODO, can't we just wait for this to happen when it happens?
            // There is a default chunk size in the protocol.
            return Err(format!("expected chunk size from client, got {:?}", msg0).into());
        }

        stream
            .send_with_options(
                RtmpMessage::WindowAcknowledgement { size: 2500000 },
                true,
                false,
            )
            .await?;

        stream
            .send(RtmpMessage::SetPeerBandwidth {
                size: 2500000,
                limit_type: PeerBandwidthLimitType::Dynamic,
            })
            .await?;

        stream
            .send(RtmpMessage::UserControl {
                event_type: UserControlEventType::StreamBegin,
                stream_id: Some(0),
                timestamp: None,
                buffer_length: None,
            })
            .await?;

        let connect_result_message = RtmpMessage::Amf0Command {
            command_name: "_result".into(),
            transaction_id: connect_trans_id,
            command_object: Amf0Value::Object(hashmap! {
                "fmsVer".into() => Amf0Value::Utf8String("ForeverTV/0.1".into()),
                // This "capabilities" claim is a lie, we can only handle h264 video and aac audio
                "capabilities".into() => Amf0Value::Number(31.0),
            }),
            additional_arguments: vec![Amf0Value::Object(hashmap! {
                "level".into() => Amf0Value::Utf8String("status".into()),
                "code".into() => Amf0Value::Utf8String("NetConnection.Connect.Success".into()),
                "description".into() => Amf0Value::Utf8String("Connection succeeded.".into()),
                "objectEncoding".into() => Amf0Value::Number(0.0),
            })],
        };
        stream.send(connect_result_message).await?;

        stream
            .send(RtmpMessage::Amf0Command {
                command_name: "onBWDone".into(),
                transaction_id: 0.0,
                command_object: Amf0Value::Null,
                additional_arguments: vec![Amf0Value::Number(8192.0)],
            })
            .await?;

        Ok(stream)
    }

    async fn send(&mut self, message: RtmpMessage) -> Result<(), Box<dyn Error>> {
        self.send_with_options(message, false, false).await
    }

    async fn send_with_options(
        &mut self,
        message: RtmpMessage,
        force_uncompressed: bool,
        can_be_dropped: bool,
    ) -> Result<(), Box<dyn Error>> {
        let payload = message
            .into_message_payload(self.clock.timestamp(), 0)
            .unwrap();
        let packet = self
            .serializer
            .serialize(&payload, force_uncompressed, can_be_dropped)
            .unwrap();

        self.client.write_all(&packet.bytes).await?;
        self.client.flush().await?;

        Ok(())
    }

    /// Blocks until the next message appears on the stream. Returns None on EOF
    async fn read_message(&mut self) -> Result<Option<(RtmpTimestamp, RtmpMessage)>, ClientError> {
        let payload = loop {
            if let Some(pending) = self.deserializer.get_next_message(&[])? {
                break pending;
            }

            let n = self.client.read(&mut self.buf).await?;
            if n == 0 {
                return Ok(None);
            }

            self.bytes_since_ack += u32::try_from(n).unwrap();
            if let Some(payload) = self.deserializer.get_next_message(&self.buf[..n])? {
                break payload;
            }
        };

        if self.bytes_since_ack >= self.ack_after_bytes {
            self.send(RtmpMessage::Acknowledgement {
                sequence_number: self.bytes_since_ack,
            })
            .await?;
            self.bytes_since_ack = 0;
        }

        let ret = payload.to_rtmp_message()?;

        Ok(Some((payload.timestamp, ret)))
    }
}

async fn handle_command(
    mut stream: ClientStream,
    command_name: String,
    transaction_id: f64,
    _command_object: Amf0Value,
    _additional_arguments: Vec<Amf0Value>,
) -> Result<ClientStream, Box<dyn Error>> {
    match command_name.as_ref() {
        "FCPublish" => {
            stream
                .send(RtmpMessage::Amf0Command {
                    command_name: "onFCPublish".into(),
                    transaction_id: 0.0,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![],
                })
                .await?;
        }
        "publish" => {
            stream
                .send(RtmpMessage::Amf0Command {
                    command_name: "onStatus".into(),
                    transaction_id: 0.0,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![Amf0Value::Object(hashmap! {
                        "level".into() => Amf0Value::Utf8String("status".into()),
                        "code".into() => Amf0Value::Utf8String("NetStream.Publish.Start".into()),
                        "description".into() => Amf0Value::Utf8String("stream is published".into()),
                        "details".into() => Amf0Value::Utf8String("no details provided".into()),
                    })],
                })
                .await?;
        }
        "createStream" => {
            stream.next_stream_id += 1.0;
            stream
                .send(RtmpMessage::Amf0Command {
                    command_name: "_result".into(),
                    transaction_id,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![Amf0Value::Number(stream.next_stream_id)],
                })
                .await?;
        }
        "releaseStream" | "_checkbw" => {
            stream
                .send(RtmpMessage::Amf0Command {
                    command_name: "_result".into(),
                    transaction_id,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![],
                })
                .await?;
        }
        "_error" | "_result" | "onStatus" | "onBWDone" => {
            eprintln!("ignoring expected message {}", command_name);
        }
        _ => {
            eprintln!("ignoring surprising message {}", command_name);
        }
    };

    Ok(stream)
}

fn handle_amf_data(
    stream: ClientStream,
    data: Vec<Amf0Value>,
) -> Result<ClientStream, Box<dyn Error>> {
    if data.len() == 3
        && data[0] == Amf0Value::Utf8String("@setDataFrame".into())
        && data[1] == Amf0Value::Utf8String("onMetaData".into())
        && matches!(data[2], Amf0Value::Object(..))
    {
        match &data[2] {
            Amf0Value::Object(metadata) => {
                eprintln!("metadata: {:?}", metadata);
            }
            _ => unreachable!(),
        }
    } else {
        eprintln!("unrecognized data {:?}", data);
    }

    Ok(stream)
}

#[derive(Debug)]
enum MediaData {
    Video {
        data: Vec<u8>,
        timestamp: i32,
        source: mixer::MixerSource,
    },
    Audio {
        data: Vec<u8>,
        timestamp: i32,
        source: mixer::MixerSource,
    },
}

async fn handle_client_stream(
    mut client_stream: ClientStream,
    source: mixer::MixerSource,
    sink: mpsc::Sender<MediaData>,
) -> Result<(), ClientError> {
    while let Some((u_timestamp, msg)) = client_stream.read_message().await? {
        // Our RTMP library doesn't allow negative timestamps, but
        // FLVs do (so our mixers do, too). We get the worst of both
        // worlds here by just failing for large inbound timestamps.
        let timestamp = i32::try_from(u_timestamp.value)?;

        match msg {
            // We need to respond to createStream
            RtmpMessage::Amf0Command {
                command_name,
                transaction_id,
                command_object,
                additional_arguments,
            } => {
                client_stream = handle_command(
                    client_stream,
                    command_name,
                    transaction_id,
                    command_object,
                    additional_arguments,
                )
                .await?;
            }
            RtmpMessage::Amf0Data { values } => {
                client_stream = handle_amf_data(client_stream, values)?;
            }
            RtmpMessage::SetChunkSize { size } => client_stream
                .deserializer
                .set_max_chunk_size(usize::try_from(size).unwrap())?,
            RtmpMessage::AudioData { data } => {
                sink.send(MediaData::Audio {
                    data: data.to_vec(), // sigh...
                    timestamp,
                    source,
                })
                .await?;
            }
            RtmpMessage::VideoData { data } => {
                sink.send(MediaData::Video {
                    data: data.to_vec(), // also sigh...
                    timestamp,
                    source,
                })
                .await?;
            }
            RtmpMessage::Acknowledgement { .. } => {
                // Ok
            }
            _ => {
                eprintln!("ignoring unexpected client message: {:?}", msg);
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("0.0.0.0:1935").await.unwrap();

    let mut out = io::stdout();
    let mut mixer = mixer::FifoMixer::default();
    let mut outbuffer = Vec::new();
    flvmux::write_flv_header(&mut outbuffer).unwrap();
    out.write_all(&outbuffer).await.unwrap();

    {
        // Scope for channels and children
        let (media_sender, mut media_receiver) = mpsc::channel(PRE_MIXER_CHANNEL_BUFFER_SIZE);
        let (client_exit_sender, mut client_exit_receiver) = mpsc::channel(MAX_CLIENT_COUNT);

        tokio::spawn(async move {
            while let Some(media) = media_receiver.recv().await {
                outbuffer.truncate(0);
                let result = match media {
                    MediaData::Video {
                        data,
                        timestamp,
                        source,
                    } => mixer.source_video(&mut outbuffer, source, &data, timestamp),
                    MediaData::Audio {
                        data,
                        timestamp,
                        source,
                    } => mixer.source_audio(&mut outbuffer, source, &data, timestamp),
                };
                result.unwrap();
                out.write_all(&outbuffer).await.unwrap(); // TODO
            }
        });

        let mut next_source: mixer::MixerSource = 0;
        let mut client_count = 0;
        loop {
            let tcp_accept = tokio::select! {
                val = listener.accept() => val,
                result = client_exit_receiver.recv() => {
                    if let Err(e) = result.unwrap() {
                        eprintln!("client error: {:?}", e);
                    }

                    client_count -= 1;
                    if client_count == 0 {
                        break;
                    } else {
                        continue;
                    }
                }
            };

            let (client, _) = tcp_accept.unwrap();
            next_source += 1;
            client_count += 1;

            let source = next_source;
            let media_snd = media_sender.clone();
            let exit_snd = client_exit_sender.clone();

            tokio::spawn(async move {
                let result = ClientStream::connect_to_client(client).await;
                let result = match result {
                    Ok(stream) => handle_client_stream(stream, source, media_snd).await,
                    Err(e) => Err(e),
                };

                exit_snd.send(result).await.unwrap();
            });
        }
    } // media_sender.drop()

    eprintln!("all writers complete.");
}
