use crate::error::{Error, Result};

pub struct Config<'a> {
    pub stereo: bool,
    pub frame: &'a [u8],
}

impl<'a> Config<'a> {
    pub fn parse(pkt: &'a [u8]) -> Result<Config<'a>> {
        if pkt.is_empty() {
            return Err(Error::BadOpus("empty packet"));
        }
        let toc = pkt[0];
        let config = toc >> 3;
        let code = toc & 3;
        if config != 31 {
            return Err(Error::Unsupported("only CELT FB 20ms (config 31)"));
        }
        if code != 0 {
            return Err(Error::Unsupported("only one frame per packet (code 0)"));
        }
        Ok(Config {
            stereo: (toc >> 2) & 1 == 1,
            frame: &pkt[1..],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config31_code0_ok() {
        let pkt = [0b11111_1_00u8, 1, 2, 3];
        let c = Config::parse(&pkt).unwrap();
        assert!(c.stereo);
        assert_eq!(c.frame, &[1, 2, 3]);
    }

    #[test]
    fn rejects_silk() {
        let pkt = [0b00000_0_00u8, 1];
        assert!(Config::parse(&pkt).is_err());
    }

    #[test]
    fn rejects_multiframe() {
        let pkt = [0b11111_0_11u8, 1];
        assert!(Config::parse(&pkt).is_err());
    }
}
