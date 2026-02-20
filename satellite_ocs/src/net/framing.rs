#[derive(Default, Clone)]
pub struct Framer;

impl Framer {
    pub fn deframe<'a>(&self, buf: &'a [u8]) -> anyhow::Result<&'a [u8]> {
        if buf.len() < 4 { anyhow::bail!("short"); }
        let len = u32::from_be_bytes([buf[0],buf[1],buf[2],buf[3]]) as usize;
        if buf.len() < 4 + len { anyhow::bail!("incomplete"); }
        Ok(&buf[..4+len])
    }

    pub fn frame(&self, data: &[u8]) -> Vec<u8> {
        let len = data.len() as u32;
        let mut framed = len.to_be_bytes().to_vec();
        framed.extend_from_slice(data);
        framed
    }
}
