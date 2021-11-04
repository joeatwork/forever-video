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
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

struct Clock(Instant);

impl Clock {
    fn timestamp(&self) -> RtmpTimestamp {
        let millis = self.0.elapsed().as_millis();
        let millis32 = millis as u32;
        RtmpTimestamp { value: millis32 }
    }
}

const BUFFER_SIZE: usize = 4096;

// Aggressively single-threaded read/writer
struct ClientStream<T: Read + Write> {
    io: T,
    serializer: ChunkSerializer,
    deserializer: ChunkDeserializer,
    clock: Clock,
    buf: [u8; BUFFER_SIZE],
    next_stream_id: f64,
    bytes_since_ack: u32,
    ack_after_bytes: u32,
}

impl<T: Read + Write> ClientStream<T> {
    fn new(io: T) -> Self {
        ClientStream {
            io,
            serializer: ChunkSerializer::new(),
            deserializer: ChunkDeserializer::new(),
            clock: Clock(Instant::now()),
            buf: [0; BUFFER_SIZE],
            next_stream_id: 3.0,
            bytes_since_ack: 0,
            ack_after_bytes: 1048576,
        }
    }

    /// Blocks until the next message appears on the stream. Returns None on EOF
    fn read_message(&mut self) -> Result<Option<RtmpMessage>, Box<dyn Error>> {
        let payload = loop {
            let n = self.io.read(&mut self.buf)?;
            if n == 0 {
                return Ok(None);
            }

            if let Some(payload) = self
                .deserializer
                .get_next_message(&self.buf[..n])
                .map_err(toerr)?
            {
                break payload;
            }

            self.bytes_since_ack += u32::try_from(n).unwrap();
        };

        // Not quite sure that this is what "ack after bytes" really means...
        // ffmpeg has some more complex logic to calculate size.
        if self.bytes_since_ack >= self.ack_after_bytes {
            self.write_message(RtmpMessage::WindowAcknowledgement {
                size: self.bytes_since_ack,
            })?;
            self.bytes_since_ack = 0;
        }

        let ret = payload.to_rtmp_message().map_err(toerr)?;
        Ok(Some(ret))
    }

    fn write_message(&mut self, msg: RtmpMessage) -> Result<(), Box<dyn Error>> {
        let payload = msg
            .into_message_payload(self.clock.timestamp(), 0)
            .map_err(toerr)?;
        let packet = self.serializer.serialize(&payload, false, false).unwrap();
        self.io.write_all(&packet.bytes)?;

        Ok(())
    }
}

fn toerr<T: Display>(error: T) -> String {
    format!("{}", error)
}

fn connect_to_client<T>(mut stream: ClientStream<T>) -> Result<ClientStream<T>, Box<dyn Error>>
where
    T: Read + Write,
{
    let mut handshake = Handshake::new(PeerType::Server);
    let hs_start = handshake.generate_outbound_p0_and_p1().unwrap();

    stream.io.write_all(&hs_start)?;

    let first_input = loop {
        let n = stream.io.read(&mut stream.buf)?;
        if n == 0 {
            // Client went away
            return Ok(stream);
        }

        let ongoing = handshake.process_bytes(&stream.buf[..n]);
        let shake_progress =
            ongoing.map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?;

        match shake_progress {
            HandshakeProcessResult::InProgress { response_bytes } => {
                stream.io.write_all(&response_bytes)?;
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                stream.io.write_all(&response_bytes)?;
                break remaining_bytes;
            }
        }
    };

    // Scan forward through everything the client says until it asks to connect.
    // This could break in a bunch of ways (the client could, for example, ask for
    // connection settings we ignore.)
    let mut input = &first_input[..];
    let connect_trans_id = loop {
        if let Some(payload) = stream.deserializer.get_next_message(input).map_err(toerr)? {
            match payload.to_rtmp_message().map_err(toerr)? {
                RtmpMessage::Amf0Command {
                    command_name,
                    transaction_id,
                    ..
                } if command_name == "connect" => {
                    break transaction_id;
                }
                other => {
                    println!(
                        "TODO skipping client rtmp message before connect {:?}",
                        other
                    )
                }
            };
        }

        let n = stream.io.read(&mut stream.buf)?;
        if n == 0 {
            return Err(Box::from("client went away before trying to connect"));
        }
        input = &stream.buf[..n];
    };

    let packet = stream
        .serializer
        .set_max_chunk_size(128, RtmpTimestamp::new(0))
        .unwrap();
    stream.io.write_all(&packet.bytes)?;

    // Header rigamarole for the client in response to connect. Taken
    // magic numbers and all from libavformat/rtmpproto.c (which is our only supported client.)
    let window_ack_message = RtmpMessage::WindowAcknowledgement { size: 2500000 };
    let window_ack_payload = window_ack_message
        .into_message_payload(stream.clock.timestamp(), 0)
        .unwrap();
    let window_ack_packet = stream
        .serializer
        .serialize(&window_ack_payload, true, false)
        .unwrap();
    stream.io.write_all(&window_ack_packet.bytes)?;

    let peer_bw_message = RtmpMessage::SetPeerBandwidth {
        size: 2500000,
        limit_type: PeerBandwidthLimitType::Dynamic,
    };
    stream.write_message(peer_bw_message)?;

    let stream_begin_message = RtmpMessage::UserControl {
        event_type: UserControlEventType::StreamBegin,
        stream_id: Some(0),
        timestamp: None,
        buffer_length: None,
    };
    stream.write_message(stream_begin_message)?;

    // Now return from the connect call
    let connect_result_message = RtmpMessage::Amf0Command {
        command_name: "_result".into(),
        transaction_id: connect_trans_id,
        command_object: Amf0Value::Object(hashmap! {
            "fmsVer".into() => Amf0Value::Utf8String("ForverTV/0.1".into()),
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
    stream.write_message(connect_result_message)?;

    let onbwdone_message = RtmpMessage::Amf0Command {
        command_name: "onBWDone".into(),
        transaction_id: 0.0,
        command_object: Amf0Value::Null,
        additional_arguments: vec![Amf0Value::Number(8192.0)],
    };
    stream.write_message(onbwdone_message)?;

    Ok(stream)
}

fn respond_to_command<T>(
    mut stream: ClientStream<T>,
    command_name: String,
    transaction_id: f64,
    _command_object: Amf0Value,
    _additional_arguments: Vec<Amf0Value>,
) -> Result<ClientStream<T>, Box<dyn Error>>
where
    T: Read + Write,
{
    match command_name.as_ref() {
        "FCPublish" => {
            stream.write_message(RtmpMessage::Amf0Command {
                command_name: "onFCPublish".into(),
                transaction_id: 0.0,
                command_object: Amf0Value::Null,
                additional_arguments: vec![],
            })?;
        }
        // Need to send onStatus(Netstream.Publish.Start)
        "publish" => {
            stream.write_message(RtmpMessage::Amf0Command {
                command_name: "onStatus".into(),
                transaction_id: 0.0,
                command_object: Amf0Value::Null,
                additional_arguments: vec![Amf0Value::Object(hashmap! {
                    "level".into() => Amf0Value::Utf8String("status".into()),
                    "code".into() => Amf0Value::Utf8String("NetStream.Publish.Start".into()),
                    "description".into() => Amf0Value::Utf8String("stream is published".into()),
                    "details".into() => Amf0Value::Utf8String("no details provided".into()),
                })],
            })?;
        }
        "createStream" => {
            stream.next_stream_id += 1.0;
            stream.write_message(RtmpMessage::Amf0Command {
                command_name: "_result".into(),
                transaction_id,
                command_object: Amf0Value::Null,
                additional_arguments: vec![Amf0Value::Number(stream.next_stream_id)],
            })?;
        }
        "releaseStream" | "_checkbw" => {
            stream.write_message(RtmpMessage::Amf0Command {
                command_name: "_result".into(),
                transaction_id,
                command_object: Amf0Value::Null,
                additional_arguments: vec![],
            })?;
        }
        "_error" | "_result" | "onStatus" | "onBWDone" => {
            println!("TODO ignoring expected message {}", command_name);
        }
        _ => {
            println!("TODO ignoring surprising message {}", command_name);
        }
    };

    Ok(stream)
}

fn main() -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind("0.0.0.0:1935").unwrap();
    let (client, _) = listener.accept()?;

    // TODO these timeouts are just for debugging, get a little more laid back for production
    client.set_read_timeout(Some(Duration::from_secs(1)))?;
    client.set_write_timeout(Some(Duration::from_secs(1)))?;

    let stream = ClientStream::new(client);
    let mut stream = connect_to_client(stream)?;

    // now just ignore all non-media messages
    while let Some(msg) = stream.read_message()? {
        match msg {
            // We need to respond to createStream
            RtmpMessage::Amf0Command {
                command_name,
                transaction_id,
                command_object,
                additional_arguments,
            } => {
                stream = respond_to_command(
                    stream,
                    command_name,
                    transaction_id,
                    command_object,
                    additional_arguments,
                )?;
            }
            RtmpMessage::SetChunkSize { size } => {
                stream
                    .deserializer
                    .set_max_chunk_size(usize::try_from(size).unwrap())
                    .map_err(toerr)?;
            }
            _ => {
                println!("TODO message: {:?}", msg);
            }
        }
    }

    // Plan
    // - Keep N threads around.
    // - for every connection, "assign" it to a thread or reject if we have too many connections.
    // - Thread - when assigned, grabs a connection, work work works, EVENTUALLY releases a connection
    println!("TODO completed cleanly");

    Ok(())
}
