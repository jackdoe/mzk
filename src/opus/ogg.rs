use crate::error::{Error, Result};

pub struct Page<'a> {
    pub granule: i64,
    pub lacing: &'a [u8],
    pub body: &'a [u8],
}

pub struct PageReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PageReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        PageReader { data, pos: 0 }
    }
}

impl<'a> Iterator for PageReader<'a> {
    type Item = Result<Page<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        let d = self.data;
        loop {
            if self.pos + 27 > d.len() {
                return None;
            }
            if &d[self.pos..self.pos + 4] != b"OggS" {
                match d[self.pos + 1..].windows(4).position(|w| w == b"OggS") {
                    Some(off) => {
                        self.pos += 1 + off;
                        continue;
                    }
                    None => return None,
                }
            }
            let nseg = d[self.pos + 26] as usize;
            let lstart = self.pos + 27;
            if lstart + nseg > d.len() {
                return None;
            }
            let lacing = &d[lstart..lstart + nseg];
            let blen: usize = lacing.iter().map(|&b| b as usize).sum();
            let bstart = lstart + nseg;
            if bstart + blen > d.len() {
                self.pos = d.len();
                return Some(Err(Error::BadOgg("truncated body")));
            }
            let granule = i64::from_le_bytes(d[self.pos + 6..self.pos + 14].try_into().unwrap());
            let body = &d[bstart..bstart + blen];
            self.pos = bstart + blen;
            return Some(Ok(Page {
                granule,
                lacing,
                body,
            }));
        }
    }
}

pub struct OpusHead {
    pub channels: u8,
    pub pre_skip: u16,
    pub output_gain: i16,
}

pub struct OpusStream {
    pub head: OpusHead,
    pub packets: Vec<Vec<u8>>,
    pub total_samples: u64,
}

const MAX_PACKET_LEN: usize = 8 << 20;
const MAX_PACKETS: usize = 1 << 20;

fn reassemble(data: &[u8]) -> Result<(Vec<Vec<u8>>, i64)> {
    let mut packets = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut last_granule = 0i64;
    for pg in PageReader::new(data) {
        let pg = pg?;
        if pg.granule >= 0 {
            last_granule = pg.granule;
        }
        let mut off = 0usize;
        for &l in pg.lacing {
            let l = l as usize;
            if cur.len() + l > MAX_PACKET_LEN {
                return Err(Error::BadOgg("packet too large"));
            }
            cur.extend_from_slice(&pg.body[off..off + l]);
            off += l;
            if l < 255 {
                if packets.len() >= MAX_PACKETS {
                    return Err(Error::BadOgg("too many packets"));
                }
                packets.push(std::mem::take(&mut cur));
            }
        }
    }
    Ok((packets, last_granule))
}

impl OpusStream {
    pub fn parse(data: &[u8]) -> Result<OpusStream> {
        let (mut packets, last_granule) = reassemble(data)?;
        if packets.len() < 2 {
            return Err(Error::BadOpus("too few packets"));
        }
        let h = &packets[0];
        if h.len() < 19 || &h[..8] != b"OpusHead" {
            return Err(Error::BadOpus("no OpusHead"));
        }
        let head = OpusHead {
            channels: h[9],
            pre_skip: u16::from_le_bytes([h[10], h[11]]),
            output_gain: i16::from_le_bytes([h[16], h[17]]),
        };
        let audio = packets.split_off(2);
        let total_samples = (last_granule as u64).saturating_sub(head.pre_skip as u64);
        Ok(OpusStream {
            head,
            packets: audio,
            total_samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_page(serial: u32, seq: u32, gran: i64, htype: u8, segs: &[u8], body: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(b"OggS");
        p.push(0);
        p.push(htype);
        p.extend_from_slice(&gran.to_le_bytes());
        p.extend_from_slice(&serial.to_le_bytes());
        p.extend_from_slice(&seq.to_le_bytes());
        p.extend_from_slice(&0u32.to_le_bytes());
        p.push(segs.len() as u8);
        p.extend_from_slice(segs);
        p.extend_from_slice(body);
        p
    }

    #[test]
    fn parses_one_page() {
        let body = [9u8; 10];
        let raw = synth_page(7, 0, 42, 0x02, &[10], &body);
        let mut r = PageReader::new(&raw);
        let pg = r.next().unwrap().unwrap();
        assert_eq!(pg.granule, 42);
        assert_eq!(pg.lacing, &[10]);
        assert_eq!(pg.body, &body[..]);
        assert!(r.next().is_none());
    }

    #[test]
    fn reads_real_corpus_head() {
        let path = match std::env::var("MZK_TEST_OPUS") {
            Ok(p) => p,
            Err(_) => return,
        };
        let data = std::fs::read(&path).unwrap();
        let stream = OpusStream::parse(&data).unwrap();
        assert_eq!(stream.head.channels, 2);
        assert_eq!(stream.head.pre_skip, 312);
        let first_audio = &stream.packets[0];
        assert_eq!(first_audio[0] >> 3, 31);
        assert!(stream.total_samples > 48000);
    }

    #[test]
    fn fuzz_page_reader_and_parse_never_panic() {
        crate::fuzz::each_case(8000, 256, |data| {
            for pg in PageReader::new(data) {
                if let Ok(p) = pg {
                    let _ = p.granule;
                    let _ = p.lacing.len();
                    let _ = p.body.len();
                }
            }
            let _ = OpusStream::parse(data);
        });
    }

    #[test]
    fn fuzz_parse_with_ogg_sync_prefix() {
        crate::fuzz::each_case(8000, 256, |data| {
            let mut framed = Vec::with_capacity(data.len() + 4);
            framed.extend_from_slice(b"OggS");
            framed.extend_from_slice(data);
            let _ = OpusStream::parse(&framed);
            for _ in PageReader::new(&framed).flatten() {}
        });
    }
}
