use tokio_util::codec::{Decoder, Encoder};
use bytes::{BytesMut, Buf};

// TODO
// replace with const_format when stable
fn get_handshake_msg() -> Vec<u8> {
    Vec::from(format!("filerep v{}", 3.0).as_bytes())
}

pub struct HandshakeEncoder {}

impl Encoder<()> for HandshakeEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, _: (), dst: &mut BytesMut) -> Result<(), Self::Error> {
        let data = get_handshake_msg();
        dst.extend_from_slice(&data[..]);
        Ok(())
    }
}

pub struct HandshakeDecoder {}

impl Decoder for HandshakeDecoder {
    type Item = ();
    type Error = std::io::Error;

    fn decode(
        &mut self,
        src: &mut BytesMut
    ) -> Result<Option<Self::Item>, Self::Error> {
        let expected = get_handshake_msg();

        if src.len() < expected.len() {
            return Ok(None);
        }

        if &src[..expected.len()] != expected {
            return Err(Self::Error::new(std::io::ErrorKind::InvalidData, "handshake failed"));
        }
        
        src.advance(expected.len());
        return Ok(Some(()));
    }
}

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;
    
    use tokio_util::codec::{Framed};
    use std::io::Cursor;
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;

    #[tokio::test]
    #[allow(unused_must_use)]
    async fn test_handshake_encdec() {
        let sink = Cursor::new(Vec::<u8>::new());

        let mut framed = Framed::new(sink, HandshakeEncoder{});
        framed.send(()).await;

        let enc = framed.into_inner().into_inner();

        let mut framed = Framed::new(Cursor::new(enc), HandshakeDecoder{});
        let dec = framed.next().await.unwrap().unwrap();

        assert_eq!(dec, ());
        let mut framed = Framed::new(Cursor::new(vec![0 as u8; 64]), HandshakeDecoder{});
        let dec = framed.next().await.unwrap();

        assert_eq!(dec.is_err(), true);
    }
}