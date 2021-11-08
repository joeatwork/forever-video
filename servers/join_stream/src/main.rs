// This file is heavily sourced from libavformat/rtmpproto.c

//   https://github.com/FFmpeg/FFmpeg/blob/05f9b3a0a570fcacbd38570f0860afdabc80a791/libavformat/rtmppkt.c
//
// As such, it is licensed under version 2.1 of the GNU Lesser General Public License,
//  or (at your option) any later version.

#[macro_use]
extern crate maplit;

use rml_amf0::Amf0Value;
use rml_rtmp::chunk_io::{ChunkDeserializer, ChunkSerializer};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::messages::{PeerBandwidthLimitType, RtmpMessage, UserControlEventType};
use rml_rtmp::time::RtmpTimestamp;
use std::convert::TryFrom;
use std::error::Error;
use std::fmt::Display;
use std::time::{Duration, Instant};
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::{tcp, TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendTimeoutError;

use flvmux::MediaType;

struct Clock(Instant);

impl Clock {
    fn timestamp(&self) -> RtmpTimestamp {
        let millis = self.0.elapsed().as_millis();
        let millis32 = millis as u32;
        RtmpTimestamp { value: millis32 }
    }
}

const BUFFER_SIZE: usize = 4096;
const WRITE_CHANNEL_SIZE: usize = 100;

#[derive(Debug)]
struct WriteMessage {
    message: RtmpMessage,
    force_uncompressed: bool,
    can_be_dropped: bool,
}

struct ClientWriter {
    sender: mpsc::Sender<WriteMessage>,
}

impl ClientWriter {
    fn new(mut output: tcp::OwnedWriteHalf) -> Self {
        let (sender, mut read_from_chan) = mpsc::channel::<WriteMessage>(WRITE_CHANNEL_SIZE);
        let clock = Clock(Instant::now());
        let mut serializer = ChunkSerializer::new();

        tokio::spawn(async move {
            while let Some(msg) = read_from_chan.recv().await {
                let payload = msg
                    .message
                    .into_message_payload(clock.timestamp(), 0)
                    .unwrap();
                let packet = serializer
                    .serialize(&payload, msg.force_uncompressed, msg.can_be_dropped)
                    .unwrap();

                if let Err(e) = output.write_all(&packet.bytes).await {
                    eprintln!("client write error: {}", e);
                    break;
                }
            }
        });

        ClientWriter { sender }
    }

    async fn send(&mut self, message: RtmpMessage) {
        self.send_with_options(message, false, false).await;
    }

    async fn send_with_options(
        &mut self,
        message: RtmpMessage,
        force_uncompressed: bool,
        can_be_dropped: bool,
    ) {
        if let Err(e) = self
            .sender
            .send_timeout(
                WriteMessage {
                    message,
                    force_uncompressed,
                    can_be_dropped,
                },
                Duration::from_secs(1),
            )
            .await
        {
            match e {
                SendTimeoutError::Closed(msg) => {
                    eprintln!("client has gone away, dropping message {:?}", &msg);
                }
                _ => panic!("{}", e),
            }
        }
    }
}

struct ClientStream {
    reader: tcp::OwnedReadHalf,
    client_writer: ClientWriter,
    deserializer: ChunkDeserializer,
    buf: [u8; BUFFER_SIZE],
    next_stream_id: f64,
    bytes_since_ack: u32,
    ack_after_bytes: u32,
}

impl ClientStream {
    async fn connect_to_client(stream: TcpStream) -> Result<ClientStream, Box<dyn Error>> {
        let (mut reader, mut writer) = stream.into_split();
        let mut buf = [0u8; BUFFER_SIZE];

        let mut handshake = Handshake::new(PeerType::Server);
        let hs_start = handshake.generate_outbound_p0_and_p1().unwrap();

        writer.write_all(&hs_start).await?;

        let first_input = loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                return Err("client went away during handshake".into());
            }

            let ongoing = handshake.process_bytes(&buf[..n]);
            let shake_progress = ongoing.map_err(toerr)?;

            match shake_progress {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    writer.write_all(&response_bytes).await?;
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    writer.write_all(&response_bytes).await?;
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
            if let Some(payload) = deserializer.get_next_message(input).map_err(toerr)? {
                match payload.to_rtmp_message().map_err(toerr)? {
                    RtmpMessage::Amf0Command {
                        command_name,
                        transaction_id,
                        ..
                    } if command_name == "connect" => {
                        break transaction_id;
                    }
                    other => {
                        eprintln!(
                            "TODO skipping client rtmp message before connect {:?}",
                            other
                        )
                    }
                };
            }

            let n = reader.read(&mut buf).await?;
            if n == 0 {
                return Err(Box::from("client went away before trying to connect"));
            }
            input = &buf[..n];
        };

        let packet = serializer
            .set_max_chunk_size(128, RtmpTimestamp::new(0))
            .unwrap();
        writer.write_all(&packet.bytes).await?;

        let mut stream = ClientStream {
            reader,
            client_writer: ClientWriter::new(writer),
            deserializer,
            buf,
            next_stream_id: 3.0,
            bytes_since_ack: 0,
            ack_after_bytes: 128,
        };

        stream
            .client_writer
            .send_with_options(
                RtmpMessage::WindowAcknowledgement { size: 2500000 },
                true,
                false,
            )
            .await;

        stream
            .client_writer
            .send(RtmpMessage::SetPeerBandwidth {
                size: 2500000,
                limit_type: PeerBandwidthLimitType::Dynamic,
            })
            .await;

        stream
            .client_writer
            .send(RtmpMessage::UserControl {
                event_type: UserControlEventType::StreamBegin,
                stream_id: Some(0),
                timestamp: None,
                buffer_length: None,
            })
            .await;

        let connect_result_message = RtmpMessage::Amf0Command {
            command_name: "_result".into(),
            transaction_id: connect_trans_id,
            command_object: Amf0Value::Object(hashmap! {
                "fmsVer".into() => Amf0Value::Utf8String("ForeverTV/0.1".into()),
                // TODO can we remove this capabilities? It's almost certainly a lie...
                "capabilities".into() => Amf0Value::Number(31.0),
            }),
            additional_arguments: vec![Amf0Value::Object(hashmap! {
                "level".into() => Amf0Value::Utf8String("status".into()),
                "code".into() => Amf0Value::Utf8String("NetConnection.Connect.Success".into()),
                "description".into() => Amf0Value::Utf8String("Connection succeeded.".into()),
                "objectEncoding".into() => Amf0Value::Number(0.0),
            })],
        };
        stream.client_writer.send(connect_result_message).await;

        stream
            .client_writer
            .send(RtmpMessage::Amf0Command {
                command_name: "onBWDone".into(),
                transaction_id: 0.0,
                command_object: Amf0Value::Null,
                additional_arguments: vec![Amf0Value::Number(8192.0)],
            })
            .await;

        Ok(stream)
    }

    /// Blocks until the next message appears on the stream. Returns None on EOF
    async fn read_message(
        &mut self,
    ) -> Result<Option<(RtmpTimestamp, RtmpMessage)>, Box<dyn Error>> {
        let payload = loop {
            let n = self.reader.read(&mut self.buf).await?;
            if n == 0 {
                return Ok(None);
            }

            self.bytes_since_ack += u32::try_from(n).unwrap();
            if let Some(payload) = self
                .deserializer
                .get_next_message(&self.buf[..n])
                .map_err(toerr)?
            {
                break payload;
            }
        };

        // TODO Not quite sure that this is what "ack after bytes" really means...
        // look more closely at the ffmpeg code and see what you really want here.
        if self.bytes_since_ack >= self.ack_after_bytes {
            self.client_writer
                .send(RtmpMessage::WindowAcknowledgement {
                    size: self.bytes_since_ack,
                })
                .await;
            self.bytes_since_ack = 0;
        }

        // Payload knows the timestamp!
        let ret = payload.to_rtmp_message().map_err(toerr)?;

        Ok(Some((payload.timestamp, ret)))
    }
}

fn toerr<T: Display>(error: T) -> String {
    format!("{}", error)
}

async fn handle_command(
    mut stream: ClientStream,
    command_name: String,
    transaction_id: f64,
    _command_object: Amf0Value,
    _additional_arguments: Vec<Amf0Value>,
) -> Result<ClientStream, Box<dyn Error>> {
    // TODO need those concurrent writes!
    match command_name.as_ref() {
        "FCPublish" => {
            stream
                .client_writer
                .send(RtmpMessage::Amf0Command {
                    command_name: "onFCPublish".into(),
                    transaction_id: 0.0,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![],
                })
                .await;
        }
        "publish" => {
            stream
                .client_writer
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
                .await;
        }
        "createStream" => {
            stream.next_stream_id += 1.0;
            stream
                .client_writer
                .send(RtmpMessage::Amf0Command {
                    command_name: "_result".into(),
                    transaction_id,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![Amf0Value::Number(stream.next_stream_id)],
                })
                .await;
        }
        "releaseStream" | "_checkbw" => {
            stream
                .client_writer
                .send(RtmpMessage::Amf0Command {
                    command_name: "_result".into(),
                    transaction_id,
                    command_object: Amf0Value::Null,
                    additional_arguments: vec![],
                })
                .await;
        }
        "_error" | "_result" | "onStatus" | "onBWDone" => {
            eprintln!("TODO ignoring expected message {}", command_name);
        }
        _ => {
            eprintln!("TODO ignoring surprising message {}", command_name);
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
        eprintln!("TODO unrecognized data {:?}", data);
    }

    Ok(stream)
}

async fn write_media_data(
    mut dest: Vec<u8>,
    media_data: &[u8],
    timestamp: RtmpTimestamp,
    media_type: MediaType,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let data_size = u32::try_from(media_data.len())?;
    let tsval = i32::try_from(timestamp.value)?;

    dest.truncate(0);
    flvmux::write_media_tag_header(&mut dest, media_type, data_size, tsval).unwrap();
    dest.write_all(media_data).await.unwrap();
    dest.write_u32(data_size + 11).await.unwrap();

    Ok(dest)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("0.0.0.0:1935").await?;
    let (client, _) = listener.accept().await?;

    let mut client_stream = ClientStream::connect_to_client(client).await?;
    let mut out = io::stdout();
    let mut outbuffer = Vec::new();
    flvmux::write_flv_header(&mut outbuffer).unwrap();
    out.write_all(&outbuffer).await.unwrap();

    // now just ignore all non-media messages
    while let Some((timestamp, msg)) = client_stream.read_message().await? {
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
            RtmpMessage::SetChunkSize { size } => {
                client_stream
                    .deserializer
                    .set_max_chunk_size(usize::try_from(size).unwrap())
                    .map_err(toerr)?;
            }
            RtmpMessage::AudioData { data } => {
                outbuffer = write_media_data(outbuffer, &data, timestamp, MediaType::Audio).await?;
                out.write_all(&outbuffer).await?;
            }
            RtmpMessage::VideoData { data } => {
                // TODO we get negative timestamps and we should detect them...
                // (maybe drop them if they're the first video data packet?)
                outbuffer = write_media_data(outbuffer, &data, timestamp, MediaType::Video).await?;
                out.write_all(&outbuffer).await?;
            }
            RtmpMessage::Acknowledgement { .. } => {
                // pass.
            }
            _ => {
                eprintln!("TODO handled message from client: {:?}", msg);
            }
        }
    }

    // Plan
    // - Keep N threads around.
    // - for every connection, "assign" it to a thread or reject if we have too many connections.
    // - Thread - when assigned, grabs a connection, work work works, EVENTUALLY releases a connection
    eprintln!("TODO completed cleanly");

    Ok(())
}
