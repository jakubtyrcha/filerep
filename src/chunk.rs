use tokio_util::codec::{Decoder, Encoder};
use bytes::{BytesMut, Buf};

pub struct FileChunk<'a> {
    pub offset : u64,
    pub data : &'a [u8]
}

pub struct FileChunkVec {
    pub offset : u64,
    pub data : Vec<u8>
}

pub struct FileChunkEncoder {}

impl<'a> Encoder<FileChunk<'a>> for FileChunkEncoder {
    type Error = std::io::Error;

    fn encode(&mut self, item: FileChunk, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let offset_slice = u64::to_le_bytes(item.offset);
        dst.extend_from_slice(&offset_slice);
        let len_slice = u32::to_le_bytes(item.data.len() as u32);
        dst.extend_from_slice(&len_slice);
        dst.extend_from_slice(item.data);
        Ok(())
    }
}

pub struct FileChunkDecoder {}

impl Decoder for FileChunkDecoder {
    type Item = FileChunkVec;
    type Error = std::io::Error;

    fn decode(
        &mut self,
        src: &mut BytesMut
    ) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 8 {
            return Ok(None);
        }
        let mut offset_bytes = [0u8; 8];
        offset_bytes.copy_from_slice(&src[..8]);
        let offset = u64::from_le_bytes(offset_bytes);

        if src.len() < 8 + 4 {
            return Ok(None);
        }
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&src[8..12]);
        let len = u32::from_le_bytes(len_bytes);

        if src.len() < 12 + len as usize {
            return Ok(None);
        }

        let data = src[12..12 + len as usize].to_vec();
        src.advance(12 + len as usize);
        return Ok(Some(FileChunkVec{ offset, data }));
    }
}

#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;
    
    use tokio_util::codec::{Framed};
    use std::io::Cursor;
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_chunk_encdec() {
        let bytes = [0 as u8, 1, 2, 3];
        let fc = FileChunk{ offset : 7, data : &bytes };
        let sink = Cursor::new(Vec::<u8>::new());

        let mut framed = Framed::new(sink, FileChunkEncoder{});
        framed.send(fc).await;

        let enc = framed.into_inner().into_inner();

        let mut framed = Framed::new(Cursor::new(enc), FileChunkDecoder{});
        let dec = framed.next().await.unwrap().unwrap();
        assert_eq!(dec.offset, 7);
        assert_eq!(dec.data.len(), bytes.len());

        for i in 0..4 {
            assert_eq!(dec.data[i], bytes[i]);
        }
    }
}