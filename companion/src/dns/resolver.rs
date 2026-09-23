use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::serialize::binary::{BinDecodable, BinDecoder};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    time::timeout,
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn decode(bytes: &[u8]) -> io::Result<Message> {
    let mut decoder = BinDecoder::new(bytes);
    let message = Message::read(&mut decoder).map_err(|_| invalid("Invalid DNS message"))?;
    if !decoder.is_empty() {
        return Err(invalid("Trailing bytes in DNS message"));
    }
    Ok(message)
}

fn validate(bytes: &[u8], query: &Message) -> io::Result<Message> {
    let message = decode(bytes)?;
    if message.id() != query.id()
        || message.message_type() != MessageType::Response
        || message.op_code() != OpCode::Query
        || message.queries().len() != 1
        || message.queries()[0] != query.queries()[0]
    {
        return Err(invalid("Upstream DNS response did not match query"));
    }
    Ok(message)
}

async fn tcp_exchange(upstream: SocketAddr, bytes: &[u8], query: &Message) -> io::Result<Message> {
    let mut stream = TcpStream::connect(upstream).await?;
    stream.write_u16(bytes.len() as u16).await?;
    stream.write_all(bytes).await?;
    let length = stream.read_u16().await?;
    if length < 12 {
        return Err(invalid("Short upstream TCP DNS response"));
    }
    let mut reply = vec![0; usize::from(length)];
    stream.read_exact(&mut reply).await?;
    let response = validate(&reply, query)?;
    if response.truncated() {
        return Err(invalid("Truncated upstream TCP DNS response"));
    }
    Ok(response)
}

async fn exchange(upstream: SocketAddr, query: &Message, tcp: bool) -> io::Result<Message> {
    let mut forwarded = query.clone();
    forwarded.set_id(rand::random());
    let bytes = forwarded
        .to_vec()
        .map_err(|_| invalid("Cannot serialize DNS query"))?;
    if tcp {
        return tcp_exchange(upstream, &bytes, &forwarded).await;
    }
    let local_ip = match upstream.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let socket = UdpSocket::bind(SocketAddr::new(local_ip, 0)).await?;
    socket.connect(upstream).await?;
    socket.send(&bytes).await?;
    let mut reply = vec![0; 65_535];
    let length = socket.recv(&mut reply).await?;
    let response = validate(&reply[..length], &forwarded)?;
    if response.truncated() {
        tcp_exchange(upstream, &bytes, &forwarded).await
    } else {
        Ok(response)
    }
}

pub(super) async fn forward(
    query: &Message,
    upstreams: &[SocketAddr],
    timeout_ms: u64,
    tcp: bool,
) -> io::Result<Message> {
    let mut last_error = io::Error::new(io::ErrorKind::NotConnected, "No DNS upstream available");
    for &upstream in upstreams {
        match timeout(
            Duration::from_millis(timeout_ms),
            exchange(upstream, query, tcp),
        )
        .await
        {
            Ok(Ok(response))
                if matches!(
                    response.response_code(),
                    ResponseCode::ServFail | ResponseCode::Refused
                ) =>
            {
                last_error = io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "DNS upstream could not answer",
                );
            }
            Ok(Ok(mut response)) => {
                response.set_id(query.id());
                return Ok(response);
            }
            Ok(Err(error)) => last_error = error,
            Err(_) => {
                last_error = io::Error::new(io::ErrorKind::TimedOut, "DNS upstream timed out")
            }
        }
    }
    Err(last_error)
}
