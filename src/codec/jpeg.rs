// Copyright (C) 2023 Niclas Olmenius <niclas@voysys.se>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [JPEG](https://www.itu.int/rec/T-REC-T.81-199209-I/en)-encoded video.
//! [RTP Payload Format for JPEG-compressed Video](https://datatracker.ietf.org/doc/html/rfc2435)

use bytes::{Buf, Bytes};

use crate::{rtp::ReceivedPacket, PacketContext, Timestamp};

use super::{VideoFrame, VideoParameters};

const MAX_FRAME_LEN: usize = 2_000_000;

#[rustfmt::skip]
const ZIGZAG : [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10,
    17, 24, 32, 25, 18, 11, 4, 5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63
];

// The following constants and functions are ported from the reference
// C code in RFC 2435 Appendix A and B.

// Appendix A. from RFC 2435

/// Table K.1 from JPEG spec.
#[rustfmt::skip]
const JPEG_LUMA_QUANTIZER: [i32; 8 * 8] = [
    16, 11, 10, 16, 24, 40, 51, 61,
    12, 12, 14, 19, 26, 58, 60, 55,
    14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77,
    24, 35, 55, 64, 81, 104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];

/// Table K.2 from JPEG spec.
#[rustfmt::skip]
const JPEG_CHROMA_QUANTIZER: [i32; 8 * 8] = [
    17, 18, 24, 47, 99, 99, 99, 99,
    18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
];

/// Calculate luma and chroma quantizer tables based on the given quality factor.
fn make_tables(q: i32) -> [u8; 128] {
    let factor = q.clamp(1, 99);
    let q = if factor < 50 {
        5000 / factor
    } else {
        200 - factor * 2
    };

    let mut qtable = [0u8; 128];
    for i in 0..64 {
        let lq = (JPEG_LUMA_QUANTIZER[ZIGZAG[i]] * q + 50) / 100;
        let cq = (JPEG_CHROMA_QUANTIZER[ZIGZAG[i]] * q + 50) / 100;

        /* Limit the quantizers to 1 <= q <= 255 */
        qtable[i] = lq.clamp(1, 255) as u8;
        qtable[i + 64] = cq.clamp(1, 255) as u8;
    }

    qtable
}

// End of Appendix A.

// Appendix B. from RFC 2435

const LUM_DC_CODELENS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const LUM_DC_SYMBOLS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const LUM_AC_CODELENS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];

#[rustfmt::skip]
const LUM_AC_SYMBOLS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
    0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
    0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
    0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa
];

const CHM_DC_CODELENS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const CHM_DC_SYMBOLS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const CHM_AC_CODELENS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];

#[rustfmt::skip]
const CHM_AC_SYMBOLS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
    0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
    0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
    0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa
];

fn make_quant_header(p: &mut Vec<u8>, qt: &[u8], table_no: u8) {
    assert!(qt.len() < (u8::MAX - 3) as usize);

    p.push(0xff);
    p.push(0xdb); // DQT
    p.push(0); // length msb
    p.push(qt.len() as u8 + 3); // length lsb
    p.push(table_no);
    p.extend_from_slice(qt);
}

fn make_huffman_header(
    p: &mut Vec<u8>,
    codelens: &[u8],
    symbols: &[u8],
    table_no: u8,
    table_class: u8,
) {
    p.push(0xff);
    p.push(0xc4); // DHT
    p.push(0); // length msb
    p.push((3 + codelens.len() + symbols.len()) as u8); // length lsb
    p.push((table_class << 4) | table_no);
    p.extend_from_slice(codelens);
    p.extend_from_slice(symbols);
}

fn make_dri_header(p: &mut Vec<u8>, dri: u16) {
    p.push(0xff);
    p.push(0xdd); // DRI
    p.push(0x0); // length msb
    p.push(4); // length lsb
    p.push((dri >> 8) as u8); // dri msb
    p.push((dri & 0xff) as u8); // dri lsb
}

fn make_headers(
    p: &mut Vec<u8>,
    image_type: u8,
    width: u16,
    height: u16,
    mut qtable: Bytes,
    precision: u8,
    dri: u16,
) -> Result<(), String> {
    p.push(0xff);
    p.push(0xd8); // SOI

    let size = if (precision & 1) > 0 { 128 } else { 64 };
    if qtable.remaining() < size {
        return Err("Qtable too small".to_string());
    }
    make_quant_header(p, &qtable[..size], 0);
    qtable.advance(size);

    let size = if (precision & 2) > 0 { 128 } else { 64 };
    if qtable.remaining() < size {
        return Err("Qtable too small".to_string());
    }
    make_quant_header(p, &qtable[..size], 1);
    qtable.advance(size);

    if dri != 0 {
        make_dri_header(p, dri);
    }

    p.push(0xff);
    p.push(0xc0); // SOF
    p.push(0); // length msb
    p.push(17); // length lsb
    p.push(8); // 8-bit precision
    p.push((height >> 8) as u8); // height msb
    p.push(height as u8); // height lsb
    p.push((width >> 8) as u8); // width msb
    p.push(width as u8); // width lsb
    p.push(3); // number of components

    p.push(0); // comp 0
    if (image_type & 0x3f) == 0 {
        p.push(0x21); // hsamp = 2, vsamp = 1
    } else {
        p.push(0x22); // hsamp = 2, vsamp = 2
    }
    p.push(0); // quant table 0

    p.push(1); // comp 1
    p.push(0x11); // hsamp = 1, vsamp = 1
    p.push(1); // quant table 1

    p.push(2); // comp 2
    p.push(0x11); // hsamp = 1, vsamp = 1
    p.push(1); // quant table 1

    make_huffman_header(p, &LUM_DC_CODELENS, &LUM_DC_SYMBOLS, 0, 0);
    make_huffman_header(p, &LUM_AC_CODELENS, &LUM_AC_SYMBOLS, 0, 1);
    make_huffman_header(p, &CHM_DC_CODELENS, &CHM_DC_SYMBOLS, 1, 0);
    make_huffman_header(p, &CHM_AC_CODELENS, &CHM_AC_SYMBOLS, 1, 1);

    p.push(0xff);
    p.push(0xda); // SOS
    p.push(0); // length msb
    p.push(12); // length lsb
    p.push(3); // 3 components

    p.push(0); // comp 0
    p.push(0); // huffman table 0

    p.push(1); // comp 1
    p.push(0x11); // huffman table 1

    p.push(2); // comp 2
    p.push(0x11); // huffman table 1

    p.push(0); // first DCT coeff
    p.push(63); // last DCT coeff
    p.push(0); // successive approx.

    Ok(())
}

// End of Appendix B.

#[derive(Debug)]
struct JpegFrameMetadata {
    start_ctx: PacketContext,
    timestamp: Timestamp,
    parameters: Option<VideoParameters>,
}

/// A [super::Depacketizer] implementation which combines fragmented RTP/JPEG
/// into complete image frames as specified in [RFC
/// 2435](https://www.rfc-editor.org/rfc/rfc2435.txt).
#[derive(Debug)]
pub struct Depacketizer {
    /// Holds metadata for the current frame.
    metadata: Option<JpegFrameMetadata>,

    /// Backing storage to the assembled frame.
    data: Vec<u8>,

    /// Cached quantization tables.
    qtables: Vec<Option<Bytes>>,

    /// A complete video frame ready for pull.
    pending: Option<VideoFrame>,

    parameters: Option<VideoParameters>,
}

impl Depacketizer {
    pub(super) fn new() -> Self {
        Depacketizer {
            metadata: None,
            data: Vec::new(),
            pending: None,
            qtables: vec![None; 255],
            parameters: None,
        }
    }

    pub(super) fn push(&mut self, pkt: ReceivedPacket) -> Result<(), String> {
        if let Some(p) = self.pending.as_ref() {
            panic!("push with data already pending: {p:?}");
        }

        if pkt.payload().len() < 8 {
            return Err("Too short RTP/JPEG packet".to_string());
        }

        let ctx = *pkt.ctx();
        let loss = pkt.loss();
        let stream_id = pkt.stream_id();
        let timestamp = pkt.timestamp();
        let last_packet_in_frame = pkt.mark();

        let mut payload = pkt.into_payload_bytes();

        //  0                   1                   2                   3
        //  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
        // | Type-specific |              Fragment Offset                  |
        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
        // |      Type     |       Q       |     Width     |     Height    |
        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
        let frag_offset = u32::from_be_bytes([0, payload[1], payload[2], payload[3]]);
        let type_specific = payload[4];
        let q = payload[5];
        let width = payload[6] as u16 * 8;
        let height = payload[7] as u16 * 8;

        let mut dri: u16 = 0;

        if frag_offset > 0 && self.metadata.is_none() {
            let _ = self.metadata.take();
            self.data.clear();

            return Err("Got JPEG fragment when we have no header".to_string());
        }

        payload.advance(8);

        if type_specific > 63 {
            if payload.remaining() < 4 {
                return Err("Too short RTP/JPEG packet".to_string());
            }

            //  0                   1                   2                   3
            //  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
            // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
            // |       Restart Interval        |F|L|       Restart Count       |
            // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
            dri = (payload[0] as u16) << 8 | payload[1] as u16;

            payload.advance(4);
        }

        if frag_offset == 0 {
            let precision;
            let qtable;

            if q >= 128 {
                if payload.len() < 4 {
                    return Err("Too short RTP/JPEG packet".to_string());
                }

                //  0                   1                   2                   3
                //  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
                // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                // |      MBZ      |   Precision   |             Length            |
                // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                // |                    Quantization Table Data                    |
                // |                              ...                              |
                // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+

                precision = payload[1];
                let length = (payload[2] as u16) << 8 | payload[3] as u16;

                payload.advance(4);

                if length as usize > payload.len() {
                    return Err(format!(
                        "Invalid RTP/JPEG packet. Length {length} larger than payload {}",
                        payload.len()
                    ));
                }

                if length == 0 {
                    // RFC 2435 section 3.1.8:
                    // "A Q value of 255 denotes that the quantization table mapping is dynamic and can change on every frame.
                    // Decoders MUST NOT depend on any previous version of the tables, and need to reload these tables on every frame.
                    // Packets MUST NOT contain Q = 255 and Length = 0."
                    if q == 255 {
                        return Err(
                            "Invalid RTP/JPEG packet. Quantization tables not found".to_string()
                        );
                    }

                    qtable = self.qtables[q as usize].clone();
                } else {
                    qtable = Some(payload.clone());
                }

                payload.advance(length as usize);
            } else {
                qtable = self.qtables[q as usize].clone().or_else(|| {
                    let table = Bytes::copy_from_slice(&make_tables(q as i32));
                    self.qtables[q as usize].replace(table);

                    self.qtables[q as usize].clone()
                });

                precision = 0;
            }

            match qtable {
                Some(qtable) => {
                    self.data.clear();

                    make_headers(
                        &mut self.data,
                        type_specific,
                        width,
                        height,
                        qtable,
                        precision,
                        dri,
                    )?;

                    self.metadata.replace(JpegFrameMetadata {
                        start_ctx: ctx,
                        timestamp,
                        parameters: Some(VideoParameters {
                            pixel_dimensions: (width as u32, height as u32),
                            rfc6381_codec: "".to_string(), // RFC 6381 is not applicable to MJPEG
                            pixel_aspect_ratio: None,
                            frame_rate: None,
                            extra_data: Bytes::new(),
                        }),
                    });
                }
                None => {
                    return Err("Invalid RTP/JPEG packet. Missing quantization tables".to_string());
                }
            }
        }

        let metadata = match &self.metadata {
            Some(metadata) => metadata,
            None => return Err("Invalid RTP/JPEG packet. Missing start packet".to_string()),
        };

        if metadata.timestamp.timestamp != timestamp.timestamp {
            // This seems to happen when you connect to certain cameras.
            // We return Ok here instead of an error to not spam the log.
            return Ok(());
        }

        self.data.extend_from_slice(&payload);

        if last_packet_in_frame {
            if self.data.len() < 2 {
                return Ok(());
            }

            // Adding EOI marker if necessary.
            let end = &self.data[self.data.len() - 2..];
            if end[0] != 0xff && end[1] != 0xd9 {
                self.data.extend_from_slice(&[0xff, 0xd9]);
            }

            let has_new_parameters = self.parameters != metadata.parameters;

            self.pending = Some(VideoFrame {
                start_ctx: metadata.start_ctx,
                end_ctx: ctx,
                has_new_parameters,
                loss,
                timestamp,
                stream_id,
                is_random_access_point: false,
                is_disposable: true,
                data: std::mem::take(&mut self.data),
            });

            let metadata = self.metadata.take();
            if let Some(metadata) = metadata {
                if has_new_parameters {
                    self.parameters = metadata.parameters;
                }
            }
        }

        if self.data.len() > MAX_FRAME_LEN {
            self.metadata = None;
            self.data.clear();
        }

        Ok(())
    }

    pub(super) fn pull(&mut self) -> Option<super::CodecItem> {
        self.pending.take().map(super::CodecItem::VideoFrame)
    }

    pub(super) fn parameters(&self) -> Option<super::ParametersRef> {
        self.parameters.as_ref().map(super::ParametersRef::Video)
    }
}

impl Default for Depacketizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use crate::testutil::init_logging;
    use crate::{codec::CodecItem, rtp::ReceivedPacketBuilder};

    // Raw RTP payload from a MJPEG encoded Big Buck Bunny stream
    // Big Buck Bunny is (c) copyright 2008, Blender Foundation, licensed via
    // Creative Commons Attribution 3.0. See https://peach.blender.org/about/

    const START_PACKET: &[u8] =
        b"\x00\x00\x00\x00\x01\xff\x28\x17\x00\x00\x00\x80\x59\x3d\x43\x4e\x43\x38\x59\x4e\
    \x48\x4e\x64\x5e\x59\x69\x85\xde\x90\x85\x7a\x7a\x85\xff\xc2\xcd\xa1\xde\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x5e\x64\x64\x85\
    \x75\x85\xff\x90\x90\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xb1\x45\x14\x50\x30\xa2\x8a\x28\x00\xa2\x8a\x28\x01\x68\xa4\xa2\x80\x0a\x28\xa2\
    \x80\x0a\x28\xa2\x80\x0a\x5a\x4a\x28\x01\x71\x45\x14\x50\x01\x45\x14\x52\x10\x52\
    \x52\xd1\x4c\x62\x51\x45\x14\x00\x52\xd2\x52\xd0\x01\x45\x14\x50\x20\xa2\x98\x1b\
    \x26\x9f\x40\x05\x14\x50\x68\x01\x0d\x1d\x69\x71\x9a\x43\xc0\x34\x00\xc6\x3d\xfd\
    \x29\x33\xcf\xaf\xbd\x21\x3c\x52\x0f\x98\xf1\x40\x85\x39\x1d\x7a\x52\xf1\x49\xfc\
    \x3c\xd2\xe7\x1f\x4a\x40\x27\x6e\xb4\xe0\x78\xeb\x4c\xe2\x97\x38\x14\x00\xa7\x83\
    \x46\xee\x3a\xd2\x67\x8a\x05\x00\x4b\x45\x14\x53\x28\x28\xa2\x8a\x00\x28\xa2\x8a\
    \x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x29\x69\x29\
    \x68\x00\xa2\x8a\x29\x00\x51\x45\x14\x00\x52\x52\xd1\x40\x05\x14\x94\x53\x01\x69\
    \xae\x70\xb4\xea\x8a\x53\x93\x8c\xd0\x21\x14\xf3\xcd\x4c\x0e\x45\x57\x07\x9e\xb5\
    \x32\x9e\x28\x12\x1d\x48\x4d\x1d\x69\xbe\xd4\xae\x02\xee\x00\xfb\x52\x33\x0f\x7a\
    \x46\x1c\x7d\x69\x98\xe7\x9e\x94\xc0\x09\xed\xd6\x81\xd2\x80\x40\xed\x9a\x0f\x22\
    \x90\x0b\xde\x93\x76\x78\xa0\x75\xa0\xf0\x73\x40\xc0\xf5\xed\x45\x26\x71\x9a\x50\
    \x78\xa0\x03\x34\xb9\x3f\x9d\x27\x7c\x52\x9c\x03\xcf\x34\x01\x2d\x14\x51\x4c\x61\
    \x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x01\
    \x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x02\xd1\x49\x4b\x48\x02\x8a\x28\xa0\
    \x04\xa2\x8a\x29\x80\x1e\x95\x01\x39\x39\xef\x52\x3e\x7a\x0a\x8b\xbd\x02\x63\x86\
    \x0e\x39\xa7\xab\x54\x63\xaf\x14\xf1\xc9\xcd\x0c\x43\xb3\xd3\x8a\x52\x39\xf4\xa4\
    \xe7\x38\x14\xbc\xe3\x9a\x90\x10\xfe\x74\xd3\xf2\xd3\xb2\x29\x87\xda\x9a\x01\xb8\
    \xe6\x9d\xdb\xad\x27\xf5\xa5\xc7\x3d\x68\x01\x33\x46\x28\xf5\xa3\x9c\x62\x81\x8a\
    \x06\x47\x14\x98\x19\xeb\x47\x3d\xff\x00\x95\x20\xa0\x07\x77\x3d\xe9\x3b\xf1\x40\
    \xc5\x2f\xe3\x40\x12\xd1\x45\x14\xc6\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\
    \x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\
    \x00\x14\x51\x45\x20\x16\x8a\x40\x68\xa0\x02\x8e\x94\x53\x59\xb0\x0f\xad\x30\x23\
    \x76\xe7\xaf\x14\xdc\x8a\x31\xc5\x00\xfb\x7e\x74\x12\x28\x39\x3e\x94\xe0\x70\x7a\
    \xe6\x98\x7e\xf7\x63\x4e\xc7\x19\xf4\xa0\x07\x82\x3d\x69\x0b\x76\xa6\xe7\x07\xad\
    \x0c\x79\xcd\x4b\x01\x69\x38\xea\x78\xa0\x62\x8a\x60\x1c\x13\x49\xf5\xa5\x07\x1d\
    \xbe\x94\x13\x9a\x00\x4c\xe2\x8c\x67\x26\x83\x80\x78\x34\x64\xfa\xd0\x30\xfa\x52\
    \xe4\x67\xbd\x27\x7a\x3a\xe6\x80\x17\xf0\xa4\xeb\x46\x48\xa4\xa0\x0b\x14\x51\x45\
    \x31\x85\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\
    \x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x04\xf1\x45\x23\x52\x01\
    \x05\x3a\x98\x3e\xb4\xa4\xfa\x51\x71\x21\x49\xc5\x44\xc4\x73\x4f\x24\x6d\xa8\x5f\
    \x93\xd6\x80\x61\x9f\xc4\xd2\x0c\x9a\x55\xe3\xde\x94\x64\x1c\x93\xcd\x31\x0a\xc4\
    \x83\x8c\x73\x49\xc7\xbd\x04\x93\xc9\xef\x49\x8a\x60\x85\x24\x11\x46\x69\x28\x06\
    \x93\x18\xf1\xed\x9a\x0d\x20\xe0\xf3\x46\x7d\x29\x08\x33\x8a\x0f\x7e\x39\xa4\x3c\
    \x1a\x33\x40\xc0\x91\x4a\x7a\x75\xa6\xe4\x03\xfc\xe9\x71\x81\x9a\x00\x33\x83\xc5\
    \x1f\x85\x1d\x68\xa0\x05\xcf\x5f\x7a\x33\x8f\xad\x21\xe0\xd1\x91\x9e\x9c\x50\x05\
    \x8a\x28\xa2\x81\x85\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x4c\
    \x02\x8a\x4c\xd1\x9a\x00\x5a\x29\x33\x46\x68\x01\x68\xa4\xcd\x19\xa0\x2e\x2d\x14\
    \x99\xa6\xb1\x34\x85\x71\xc4\x8a\x63\x36\x38\xef\x46\x70\x0e\x48\x22\x99\xbb\x1d\
    \x0d\x21\x0a\x49\xe9\x4b\xf9\xd3\x73\xf3\x7f\x5a\x39\xce\x73\x40\x0a\x73\x9f\xe9\
    \x51\x9c\x67\xde\x9c\x49\xa6\x0f\x7e\x94\xd0\x0b\x9c\x73\xde\x9c\xbc\x8e\x7a\xd3\
    \x3b\x53\xb3\x8e\x84\xd3\x00\xc1\x39\xe6\x8a\x50\x47\x7e\x69\x0b\x03\xd0\x71\x4c\
    \x04\xcd\x02\x92\x96\x91\x43\xf3\xc7\x1d\x0d\x27\x4a\x07\x4a\x0d\x21\x01\xe3\x39\
    \xa3\xd7\x02\x90\xf4\xa5\x04\xe3\x02\x80\x00\x09\x14\x7a\xf6\xa4\xc6\x4d\x03\x8a\
    \x00\x5c\xd2\x0a\x0f\xb5\x00\xf6\xa0\x05\xcf\xcb\xfd\x69\x33\xc6\x29\x7a\x03\x9a\
    \x6e\x41\xef\x40\x16\xa8\xa6\xee\xa6\x92\x4d\x3b\x05\xc9\x29\x32\x29\x94\x53\xb0\
    \x5c\x76\xea\x37\x53\x68\xa2\xc2\xb8\xed\xd4\x66\x9b\x45\x01\x71\xd9\xa3\x34\xda\
    \x28\x01\x73\x46\x69\x28\xa0\x05\xcd\x2e\x69\xb4\x50\x03\xa8\xa6\xd1\x40\x03\x74\
    \xe0\xd3\x41\xcf\x1f\x8d\x38\xd3\x57\x81\x9e\xf5\x22\x1b\x9c\x1c\xfe\x06\x94\x8c\
    \x74\x5e\x29\x32\x73\x9a\x5e\x9c\xd0\x31\x0e\x32\x39\xe2\x83\x49\xe9\x4a\x0e\x78\
    \xcd\x00\x27\x34\x98\xf5\x34\xa7\x26\x93\x93\x4d\x00\xa7\x1d\x05\x07\xaf\x1c\x50\
    \x0f\x7e\xf4\x80\x9e\x69\x80\xa3\x20\x67\x9e\x69\xbf\x5a\x5d\xc7\xa5\x18\xcf\x22\
    \x80\x12\x94\x52\x51\x9e\x69\x0c\x7a\xfa\x0a\x0e\x69\xb9\xe7\x34\xec\xe3\x9e\xb4\
    \x84\x1c\x63\x14\xda\x77\x38\xe2\x90\xd0\x01\x8f\x4a\x06\x7a\xe7\x8a\x0f\xa5\x1c\
    \xd0\x00\x7d\x7b\x51\xd0\x0a\x4a\x19\xb9\xc7\xf2\xa0\x03\x23\xa7\x6a\x38\xa3\x39\
    \xc5\x2f\x5e\x78\x14\xc0\x97\x9a\x4e\x69\xd8\x38\xcd\x25\x4f\x30\x09\xcf\xa5\x1f\
    \x85\x2d\x2d\x1c\xc0\x37\x9a\x39\xa7\x51\x4b\x98\x06\xf3\x46\x0d\x3b\x14\x62\x8e\
    \x70\xb0\x9c\xd2\x73\x4e\xc1\xa4\xa7\xcc\x16\x13\x9f\x6a\x39\xa5\xa2\x8b\x80\x98\
    \x34\xbc\xd1\x40\xa2\xe0\x1c\xd0\x33\xde\x94\x8c\x51\x9e\x0d\x3b\x80\xd2\x4f\x4e\
    \x94\x8c\x78\x14\xa3\x1d\xfa\xd3\x4f\x7f\x4a\x04\x27\xf5\xa3\xa0\xeb\xc5\x1d\x28\
    \xe9\x40\xc4\xcd\x25\x2f\x14\x76\xa6\x02\x62\x8e\x3d\x68\xce\x28\xce\x7d\xa8\x00\
    \xc8\xec\x28\xce\x17\x34\xa0\x6e";

    const END_PACKET: &[u8] =
        b"\x00\x00\x04\xe0\x01\xff\x28\x17\x3e\xd4\xd3\xc9\xa0\x03\x8a\x76\x70\x3a\xd3\x71\
    \xc5\x14\xc0\x29\x29\x49\x1d\xa9\x28\x18\xa2\x94\x51\x8e\x28\xc5\x48\x07\x38\xc7\
    \xbd\x38\xf2\x7a\xfb\x52\x62\x90\x2f\x27\x3d\x28\x10\xbd\x0d\x21\xe3\x22\x9c\x38\
    \x14\x87\x9a\x00\x6b\x1e\x29\x83\x93\x4f\x2a\x4f\x7a\x02\xe2\x9d\xc6\x34\xd3\xb2\
    \x71\x46\xda\x36\x9a\x00\xb1\x9a\x5e\x29\xb9\xcf\x4a\x2b\x20\x1d\xc5\x18\xa6\x67\
    \xda\x9c\x1a\x80\x0c\x51\x40\x34\x13\x8a\x00\x5a\x29\x37\x71\x9a\x5c\xd2\x01\x73\
    \x40\x34\x99\x06\x97\x8a\x68\x03\x00\xd2\x6d\xa2\x8c\xd0\x02\x60\xd2\x60\xd3\xb3\
    \x45\x00\x20\x3c\x52\x52\x93\x8a\x4c\x8a\x68\x04\x3f\xa5\x30\xe2\x9f\x91\xed\x46\
    \x57\xda\xaa\xe2\xb1\x1f\x5a\x5e\x48\xa7\x6e\x14\x6e\x14\x5c\x2c\x37\x69\xf4\xa3\
    \x6d\x3b\x75\x26\xea\x2e\xc6\x34\xa1\xa5\x09\xeb\x46\xea\x37\x51\x76\x02\xe3\x02\
    \x93\x68\xf5\xa4\xcd\x19\x34\x6a\x2b\x0b\x81\x46\x05\x25\x14\x0c\x5e\x28\xcd\x25\
    \x14\x00\xb9\xa2\x92\x8a\x00\x28\xa2\x92\x80\x17\x34\x66\x92\x8a\x00\x5c\xd2\x51\
    \x45\x00\x14\x51\x47\x5a\x00\x95\x40\xed\x4e\x09\xef\x48\x30\x38\x14\xf0\x73\x4a\
    \xc0\x31\xc1\x1e\xf4\xd1\xc5\x4b\xd7\x83\x51\x9c\x86\x22\x90\xc3\x34\x13\xc6\x0d\
    \x21\xce\x7a\xd0\x0f\x3c\xd0\x21\x78\x34\xbc\x53\x73\x8e\xbd\xe8\xc8\xa2\xc0\x3b\
    \x00\x1a\x5e\xb4\xdc\xf7\xa3\x38\x34\x80\x77\x4a\x33\x4d\x2d\x46\x28\x01\xf9\x14\
    \x99\x14\x94\x01\xea\x68\xb0\x0e\xeb\x4d\x2a\x0f\x6a\x09\x51\xde\x94\x36\x29\xa4\
    \x02\x79\x74\x9b\x29\xfb\xc5\x21\x3d\xe8\x01\xbb\x68\xdb\x4b\x91\x4b\x45\xc0\x66\
    \xda\x0a\xd3\xe9\x30\x0d\x17\x01\x98\xa5\xdb\x4e\xa3\x8a\x2e\x03\x08\xa4\x34\xf2\
    \x3d\x29\x30\x7a\x51\x70\x1b\x8a\x31\xef\x4e\xc5\x04\x76\xc5\x3b\x80\xde\x28\xc0\
    \xa7\x05\xe6\x8c\x62\x8b\x80\xdc\x51\x4f\x55\xeb\x9a\x71\xe4\x12\xc2\x80\x21\xc5\
    \x18\xa7\x6d\x20\x52\x50\x02\x63\x9a\x31\x4b\x83\x9e\x94\x11\x9a\x00\x42\x05\x26\
    \x29\xd8\xed\x49\xd2\x98\x09\xf8\x51\x4b\xd6\x8f\xc0\xd0\x21\xfd\x09\xa5\x07\xd2\
    \x8c\x0c\xd0\x70\x29\x5c\x64\x80\xd3\x1c\xf2\x31\x4d\xa5\xa4\x30\x1c\x0a\x4c\x52\
    \xf7\xa0\xd2\x10\x6d\xcb\x52\x11\x8e\x94\xb9\x3f\x85\x07\x8a\x60\x27\x22\x82\x38\
    \xa3\x3e\xb4\x13\x40\x09\xb4\xd2\xf3\x49\xba\x97\x3c\x50\x02\xe7\x02\x93\x34\xb4\
    \xda\x00\x53\x47\x5a\x42\x38\xcf\xbd\x28\xe3\xf2\xa0\x04\xe9\x4e\xce\x05\x37\x34\
    \xb8\xc8\xcd\x3b\x80\xa0\x8c\x66\x80\x72\x28\xc5\x18\xa4\x02\xf3\x8a\x39\xa4\x3e\
    \x94\xbd\x29\x00\xde\x77\x52\xf3\x47\x19\xa0\x9a\x00\x0e\x69\x73\xde\x93\x9a\x30\
    \x73\x4c\x05\x07\x8a\x33\xcd\x20\x1e\xf4\x52\x01\x41\xf5\xa1\x73\xd6\x8e\x7a\x0a\
    \x4c\x9e\xd4\xc0\x50\x4f\x7a\x5c\xe7\xd4\xd2\x1e\x29\x03\x73\x43\x60\x29\x23\x3f\
    \xd2\x8f\xc2\x8c\xe7\xad\x26\x06\x29\x00\xb4\x64\x53\x76\x9c\x52\xfd\x28\x01\x78\
    \xa6\xec\x18\x34\xb4\x03\x4c\x05\x2a\x00\xe2\x93\x00\xf5\xa0\x67\x18\xa0\x67\x34\
    \x00\x83\xa5\x29\xe8\x68\xa2\x90\x05\x1d\xa8\xa2\x80\x14\x52\x35\x14\x50\x02\x03\
    \xc5\x2f\x51\x45\x14\xc0\x43\x4c\x6a\x28\xa0\x00\x0e\x4d\x3f\xb5\x14\x53\x60\x20\
    \xef\x4a\xbc\x9a\x28\xa4\x00\x4e\x28\xef\x45\x14\x80\x50\x29\x68\xa2\x80\x00\x78\
    \xa2\x8a\x28\x01\x29\x28\xa2\x80\x0f\x43\x4b\xfc\x54\x51\x4c\x07\x52\x76\xa2\x8a\
    \x40\x21\xa4\xc9\xa2\x8a\x00\x71\xeb\x45\x14\x50\x03\x4f\x6a\x31\xc8\xa2\x8a\x60\
    \x1f\xe3\x4a\x4f\x02\x8a\x28\x00\xcf\x14\xb9\xe6\x8a\x29\x00\x51\xd2\x8a\x28\x00\
    \x3c\x52\x03\x45\x14\x01\xff\xd9";

    const VALID_JPEG_IMAGE: &[u8] =
        b"\xff\xd8\xff\xdb\x00\x43\x00\x59\x3d\x43\x4e\x43\x38\x59\x4e\x48\x4e\x64\x5e\x59\
    \x69\x85\xde\x90\x85\x7a\x7a\x85\xff\xc2\xcd\xa1\xde\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xdb\x00\x43\x01\x5e\x64\x64\x85\
    \x75\x85\xff\x90\x90\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
    \xff\xc0\x00\x11\x08\x00\xb8\x01\x40\x03\x00\x22\x00\x01\x11\x01\x02\x11\x01\xff\
    \xc4\x00\x1f\x00\x00\x01\x05\x01\x01\x01\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\
    \x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\xff\xc4\x00\xb5\x10\x00\x02\x01\
    \x03\x03\x02\x04\x03\x05\x05\x04\x04\x00\x00\x01\x7d\x01\x02\x03\x00\x04\x11\x05\
    \x12\x21\x31\x41\x06\x13\x51\x61\x07\x22\x71\x14\x32\x81\x91\xa1\x08\x23\x42\xb1\
    \xc1\x15\x52\xd1\xf0\x24\x33\x62\x72\x82\x09\x0a\x16\x17\x18\x19\x1a\x25\x26\x27\
    \x28\x29\x2a\x34\x35\x36\x37\x38\x39\x3a\x43\x44\x45\x46\x47\x48\x49\x4a\x53\x54\
    \x55\x56\x57\x58\x59\x5a\x63\x64\x65\x66\x67\x68\x69\x6a\x73\x74\x75\x76\x77\x78\
    \x79\x7a\x83\x84\x85\x86\x87\x88\x89\x8a\x92\x93\x94\x95\x96\x97\x98\x99\x9a\xa2\
    \xa3\xa4\xa5\xa6\xa7\xa8\xa9\xaa\xb2\xb3\xb4\xb5\xb6\xb7\xb8\xb9\xba\xc2\xc3\xc4\
    \xc5\xc6\xc7\xc8\xc9\xca\xd2\xd3\xd4\xd5\xd6\xd7\xd8\xd9\xda\xe1\xe2\xe3\xe4\xe5\
    \xe6\xe7\xe8\xe9\xea\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9\xfa\xff\xc4\x00\x1f\x01\
    \x00\x03\x01\x01\x01\x01\x01\x01\x01\x01\x01\x00\x00\x00\x00\x00\x00\x01\x02\x03\
    \x04\x05\x06\x07\x08\x09\x0a\x0b\xff\xc4\x00\xb5\x11\x00\x02\x01\x02\x04\x04\x03\
    \x04\x07\x05\x04\x04\x00\x01\x02\x77\x00\x01\x02\x03\x11\x04\x05\x21\x31\x06\x12\
    \x41\x51\x07\x61\x71\x13\x22\x32\x81\x08\x14\x42\x91\xa1\xb1\xc1\x09\x23\x33\x52\
    \xf0\x15\x62\x72\xd1\x0a\x16\x24\x34\xe1\x25\xf1\x17\x18\x19\x1a\x26\x27\x28\x29\
    \x2a\x35\x36\x37\x38\x39\x3a\x43\x44\x45\x46\x47\x48\x49\x4a\x53\x54\x55\x56\x57\
    \x58\x59\x5a\x63\x64\x65\x66\x67\x68\x69\x6a\x73\x74\x75\x76\x77\x78\x79\x7a\x82\
    \x83\x84\x85\x86\x87\x88\x89\x8a\x92\x93\x94\x95\x96\x97\x98\x99\x9a\xa2\xa3\xa4\
    \xa5\xa6\xa7\xa8\xa9\xaa\xb2\xb3\xb4\xb5\xb6\xb7\xb8\xb9\xba\xc2\xc3\xc4\xc5\xc6\
    \xc7\xc8\xc9\xca\xd2\xd3\xd4\xd5\xd6\xd7\xd8\xd9\xda\xe2\xe3\xe4\xe5\xe6\xe7\xe8\
    \xe9\xea\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9\xfa\xff\xda\x00\x0c\x03\x00\x00\x01\x11\
    \x02\x11\x00\x3f\x00\xb1\x45\x14\x50\x30\xa2\x8a\x28\x00\xa2\x8a\x28\x01\x68\xa4\
    \xa2\x80\x0a\x28\xa2\x80\x0a\x28\xa2\x80\x0a\x5a\x4a\x28\x01\x71\x45\x14\x50\x01\
    \x45\x14\x52\x10\x52\x52\xd1\x4c\x62\x51\x45\x14\x00\x52\xd2\x52\xd0\x01\x45\x14\
    \x50\x20\xa2\x98\x1b\x26\x9f\x40\x05\x14\x50\x68\x01\x0d\x1d\x69\x71\x9a\x43\xc0\
    \x34\x00\xc6\x3d\xfd\x29\x33\xcf\xaf\xbd\x21\x3c\x52\x0f\x98\xf1\x40\x85\x39\x1d\
    \x7a\x52\xf1\x49\xfc\x3c\xd2\xe7\x1f\x4a\x40\x27\x6e\xb4\xe0\x78\xeb\x4c\xe2\x97\
    \x38\x14\x00\xa7\x83\x46\xee\x3a\xd2\x67\x8a\x05\x00\x4b\x45\x14\x53\x28\x28\xa2\
    \x8a\x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x28\xa2\x8a\x00\x28\xa2\
    \x8a\x00\x29\x69\x29\x68\x00\xa2\x8a\x29\x00\x51\x45\x14\x00\x52\x52\xd1\x40\x05\
    \x14\x94\x53\x01\x69\xae\x70\xb4\xea\x8a\x53\x93\x8c\xd0\x21\x14\xf3\xcd\x4c\x0e\
    \x45\x57\x07\x9e\xb5\x32\x9e\x28\x12\x1d\x48\x4d\x1d\x69\xbe\xd4\xae\x02\xee\x00\
    \xfb\x52\x33\x0f\x7a\x46\x1c\x7d\x69\x98\xe7\x9e\x94\xc0\x09\xed\xd6\x81\xd2\x80\
    \x40\xed\x9a\x0f\x22\x90\x0b\xde\x93\x76\x78\xa0\x75\xa0\xf0\x73\x40\xc0\xf5\xed\
    \x45\x26\x71\x9a\x50\x78\xa0\x03\x34\xb9\x3f\x9d\x27\x7c\x52\x9c\x03\xcf\x34\x01\
    \x2d\x14\x51\x4c\x61\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\
    \x01\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x01\x45\x14\x50\x02\xd1\x49\x4b\
    \x48\x02\x8a\x28\xa0\x04\xa2\x8a\x29\x80\x1e\x95\x01\x39\x39\xef\x52\x3e\x7a\x0a\
    \x8b\xbd\x02\x63\x86\x0e\x39\xa7\xab\x54\x63\xaf\x14\xf1\xc9\xcd\x0c\x43\xb3\xd3\
    \x8a\x52\x39\xf4\xa4\xe7\x38\x14\xbc\xe3\x9a\x90\x10\xfe\x74\xd3\xf2\xd3\xb2\x29\
    \x87\xda\x9a\x01\xb8\xe6\x9d\xdb\xad\x27\xf5\xa5\xc7\x3d\x68\x01\x33\x46\x28\xf5\
    \xa3\x9c\x62\x81\x8a\x06\x47\x14\x98\x19\xeb\x47\x3d\xff\x00\x95\x20\xa0\x07\x77\
    \x3d\xe9\x3b\xf1\x40\xc5\x2f\xe3\x40\x12\xd1\x45\x14\xc6\x14\x51\x45\x00\x14\x51\
    \x45\x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\x45\x00\x14\x51\
    \x45\x00\x14\x51\x45\x00\x14\x51\x45\x20\x16\x8a\x40\x68\xa0\x02\x8e\x94\x53\x59\
    \xb0\x0f\xad\x30\x23\x76\xe7\xaf\x14\xdc\x8a\x31\xc5\x00\xfb\x7e\x74\x12\x28\x39\
    \x3e\x94\xe0\x70\x7a\xe6\x98\x7e\xf7\x63\x4e\xc7\x19\xf4\xa0\x07\x82\x3d\x69\x0b\
    \x76\xa6\xe7\x07\xad\x0c\x79\xcd\x4b\x01\x69\x38\xea\x78\xa0\x62\x8a\x60\x1c\x13\
    \x49\xf5\xa5\x07\x1d\xbe\x94\x13\x9a\x00\x4c\xe2\x8c\x67\x26\x83\x80\x78\x34\x64\
    \xfa\xd0\x30\xfa\x52\xe4\x67\xbd\x27\x7a\x3a\xe6\x80\x17\xf0\xa4\xeb\x46\x48\xa4\
    \xa0\x0b\x14\x51\x45\x31\x85\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\
    \x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\x40\x05\x04\
    \xf1\x45\x23\x52\x01\x05\x3a\x98\x3e\xb4\xa4\xfa\x51\x71\x21\x49\xc5\x44\xc4\x73\
    \x4f\x24\x6d\xa8\x5f\x93\xd6\x80\x61\x9f\xc4\xd2\x0c\x9a\x55\xe3\xde\x94\x64\x1c\
    \x93\xcd\x31\x0a\xc4\x83\x8c\x73\x49\xc7\xbd\x04\x93\xc9\xef\x49\x8a\x60\x85\x24\
    \x11\x46\x69\x28\x06\x93\x18\xf1\xed\x9a\x0d\x20\xe0\xf3\x46\x7d\x29\x08\x33\x8a\
    \x0f\x7e\x39\xa4\x3c\x1a\x33\x40\xc0\x91\x4a\x7a\x75\xa6\xe4\x03\xfc\xe9\x71\x81\
    \x9a\x00\x33\x83\xc5\x1f\x85\x1d\x68\xa0\x05\xcf\x5f\x7a\x33\x8f\xad\x21\xe0\xd1\
    \x91\x9e\x9c\x50\x05\x8a\x28\xa2\x81\x85\x14\x51\x40\x05\x14\x51\x40\x05\x14\x51\
    \x40\x05\x14\x51\x4c\x02\x8a\x4c\xd1\x9a\x00\x5a\x29\x33\x46\x68\x01\x68\xa4\xcd\
    \x19\xa0\x2e\x2d\x14\x99\xa6\xb1\x34\x85\x71\xc4\x8a\x63\x36\x38\xef\x46\x70\x0e\
    \x48\x22\x99\xbb\x1d\x0d\x21\x0a\x49\xe9\x4b\xf9\xd3\x73\xf3\x7f\x5a\x39\xce\x73\
    \x40\x0a\x73\x9f\xe9\x51\x9c\x67\xde\x9c\x49\xa6\x0f\x7e\x94\xd0\x0b\x9c\x73\xde\
    \x9c\xbc\x8e\x7a\xd3\x3b\x53\xb3\x8e\x84\xd3\x00\xc1\x39\xe6\x8a\x50\x47\x7e\x69\
    \x0b\x03\xd0\x71\x4c\x04\xcd\x02\x92\x96\x91\x43\xf3\xc7\x1d\x0d\x27\x4a\x07\x4a\
    \x0d\x21\x01\xe3\x39\xa3\xd7\x02\x90\xf4\xa5\x04\xe3\x02\x80\x00\x09\x14\x7a\xf6\
    \xa4\xc6\x4d\x03\x8a\x00\x5c\xd2\x0a\x0f\xb5\x00\xf6\xa0\x05\xcf\xcb\xfd\x69\x33\
    \xc6\x29\x7a\x03\x9a\x6e\x41\xef\x40\x16\xa8\xa6\xee\xa6\x92\x4d\x3b\x05\xc9\x29\
    \x32\x29\x94\x53\xb0\x5c\x76\xea\x37\x53\x68\xa2\xc2\xb8\xed\xd4\x66\x9b\x45\x01\
    \x71\xd9\xa3\x34\xda\x28\x01\x73\x46\x69\x28\xa0\x05\xcd\x2e\x69\xb4\x50\x03\xa8\
    \xa6\xd1\x40\x03\x74\xe0\xd3\x41\xcf\x1f\x8d\x38\xd3\x57\x81\x9e\xf5\x22\x1b\x9c\
    \x1c\xfe\x06\x94\x8c\x74\x5e\x29\x32\x73\x9a\x5e\x9c\xd0\x31\x0e\x32\x39\xe2\x83\
    \x49\xe9\x4a\x0e\x78\xcd\x00\x27\x34\x98\xf5\x34\xa7\x26\x93\x93\x4d\x00\xa7\x1d\
    \x05\x07\xaf\x1c\x50\x0f\x7e\xf4\x80\x9e\x69\x80\xa3\x20\x67\x9e\x69\xbf\x5a\x5d\
    \xc7\xa5\x18\xcf\x22\x80\x12\x94\x52\x51\x9e\x69\x0c\x7a\xfa\x0a\x0e\x69\xb9\xe7\
    \x34\xec\xe3\x9e\xb4\x84\x1c\x63\x14\xda\x77\x38\xe2\x90\xd0\x01\x8f\x4a\x06\x7a\
    \xe7\x8a\x0f\xa5\x1c\xd0\x00\x7d\x7b\x51\xd0\x0a\x4a\x19\xb9\xc7\xf2\xa0\x03\x23\
    \xa7\x6a\x38\xa3\x39\xc5\x2f\x5e\x78\x14\xc0\x97\x9a\x4e\x69\xd8\x38\xcd\x25\x4f\
    \x30\x09\xcf\xa5\x1f\x85\x2d\x2d\x1c\xc0\x37\x9a\x39\xa7\x51\x4b\x98\x06\xf3\x46\
    \x0d\x3b\x14\x62\x8e\x70\xb0\x9c\xd2\x73\x4e\xc1\xa4\xa7\xcc\x16\x13\x9f\x6a\x39\
    \xa5\xa2\x8b\x80\x98\x34\xbc\xd1\x40\xa2\xe0\x1c\xd0\x33\xde\x94\x8c\x51\x9e\x0d\
    \x3b\x80\xd2\x4f\x4e\x94\x8c\x78\x14\xa3\x1d\xfa\xd3\x4f\x7f\x4a\x04\x27\xf5\xa3\
    \xa0\xeb\xc5\x1d\x28\xe9\x40\xc4\xcd\x25\x2f\x14\x76\xa6\x02\x62\x8e\x3d\x68\xce\
    \x28\xce\x7d\xa8\x00\xc8\xec\x28\xce\x17\x34\xa0\x6e\x3e\xd4\xd3\xc9\xa0\x03\x8a\
    \x76\x70\x3a\xd3\x71\xc5\x14\xc0\x29\x29\x49\x1d\xa9\x28\x18\xa2\x94\x51\x8e\x28\
    \xc5\x48\x07\x38\xc7\xbd\x38\xf2\x7a\xfb\x52\x62\x90\x2f\x27\x3d\x28\x10\xbd\x0d\
    \x21\xe3\x22\x9c\x38\x14\x87\x9a\x00\x6b\x1e\x29\x83\x93\x4f\x2a\x4f\x7a\x02\xe2\
    \x9d\xc6\x34\xd3\xb2\x71\x46\xda\x36\x9a\x00\xb1\x9a\x5e\x29\xb9\xcf\x4a\x2b\x20\
    \x1d\xc5\x18\xa6\x67\xda\x9c\x1a\x80\x0c\x51\x40\x34\x13\x8a\x00\x5a\x29\x37\x71\
    \x9a\x5c\xd2\x01\x73\x40\x34\x99\x06\x97\x8a\x68\x03\x00\xd2\x6d\xa2\x8c\xd0\x02\
    \x60\xd2\x60\xd3\xb3\x45\x00\x20\x3c\x52\x52\x93\x8a\x4c\x8a\x68\x04\x3f\xa5\x30\
    \xe2\x9f\x91\xed\x46\x57\xda\xaa\xe2\xb1\x1f\x5a\x5e\x48\xa7\x6e\x14\x6e\x14\x5c\
    \x2c\x37\x69\xf4\xa3\x6d\x3b\x75\x26\xea\x2e\xc6\x34\xa1\xa5\x09\xeb\x46\xea\x37\
    \x51\x76\x02\xe3\x02\x93\x68\xf5\xa4\xcd\x19\x34\x6a\x2b\x0b\x81\x46\x05\x25\x14\
    \x0c\x5e\x28\xcd\x25\x14\x00\xb9\xa2\x92\x8a\x00\x28\xa2\x92\x80\x17\x34\x66\x92\
    \x8a\x00\x5c\xd2\x51\x45\x00\x14\x51\x47\x5a\x00\x95\x40\xed\x4e\x09\xef\x48\x30\
    \x38\x14\xf0\x73\x4a\xc0\x31\xc1\x1e\xf4\xd1\xc5\x4b\xd7\x83\x51\x9c\x86\x22\x90\
    \xc3\x34\x13\xc6\x0d\x21\xce\x7a\xd0\x0f\x3c\xd0\x21\x78\x34\xbc\x53\x73\x8e\xbd\
    \xe8\xc8\xa2\xc0\x3b\x00\x1a\x5e\xb4\xdc\xf7\xa3\x38\x34\x80\x77\x4a\x33\x4d\x2d\
    \x46\x28\x01\xf9\x14\x99\x14\x94\x01\xea\x68\xb0\x0e\xeb\x4d\x2a\x0f\x6a\x09\x51\
    \xde\x94\x36\x29\xa4\x02\x79\x74\x9b\x29\xfb\xc5\x21\x3d\xe8\x01\xbb\x68\xdb\x4b\
    \x91\x4b\x45\xc0\x66\xda\x0a\xd3\xe9\x30\x0d\x17\x01\x98\xa5\xdb\x4e\xa3\x8a\x2e\
    \x03\x08\xa4\x34\xf2\x3d\x29\x30\x7a\x51\x70\x1b\x8a\x31\xef\x4e\xc5\x04\x76\xc5\
    \x3b\x80\xde\x28\xc0\xa7\x05\xe6\x8c\x62\x8b\x80\xdc\x51\x4f\x55\xeb\x9a\x71\xe4\
    \x12\xc2\x80\x21\xc5\x18\xa7\x6d\x20\x52\x50\x02\x63\x9a\x31\x4b\x83\x9e\x94\x11\
    \x9a\x00\x42\x05\x26\x29\xd8\xed\x49\xd2\x98\x09\xf8\x51\x4b\xd6\x8f\xc0\xd0\x21\
    \xfd\x09\xa5\x07\xd2\x8c\x0c\xd0\x70\x29\x5c\x64\x80\xd3\x1c\xf2\x31\x4d\xa5\xa4\
    \x30\x1c\x0a\x4c\x52\xf7\xa0\xd2\x10\x6d\xcb\x52\x11\x8e\x94\xb9\x3f\x85\x07\x8a\
    \x60\x27\x22\x82\x38\xa3\x3e\xb4\x13\x40\x09\xb4\xd2\xf3\x49\xba\x97\x3c\x50\x02\
    \xe7\x02\x93\x34\xb4\xda\x00\x53\x47\x5a\x42\x38\xcf\xbd\x28\xe3\xf2\xa0\x04\xe9\
    \x4e\xce\x05\x37\x34\xb8\xc8\xcd\x3b\x80\xa0\x8c\x66\x80\x72\x28\xc5\x18\xa4\x02\
    \xf3\x8a\x39\xa4\x3e\x94\xbd\x29\x00\xde\x77\x52\xf3\x47\x19\xa0\x9a\x00\x0e\x69\
    \x73\xde\x93\x9a\x30\x73\x4c\x05\x07\x8a\x33\xcd\x20\x1e\xf4\x52\x01\x41\xf5\xa1\
    \x73\xd6\x8e\x7a\x0a\x4c\x9e\xd4\xc0\x50\x4f\x7a\x5c\xe7\xd4\xd2\x1e\x29\x03\x73\
    \x43\x60\x29\x23\x3f\xd2\x8f\xc2\x8c\xe7\xad\x26\x06\x29\x00\xb4\x64\x53\x76\x9c\
    \x52\xfd\x28\x01\x78\xa6\xec\x18\x34\xb4\x03\x4c\x05\x2a\x00\xe2\x93\x00\xf5\xa0\
    \x67\x18\xa0\x67\x34\x00\x83\xa5\x29\xe8\x68\xa2\x90\x05\x1d\xa8\xa2\x80\x14\x52\
    \x35\x14\x50\x02\x03\xc5\x2f\x51\x45\x14\xc0\x43\x4c\x6a\x28\xa0\x00\x0e\x4d\x3f\
    \xb5\x14\x53\x60\x20\xef\x4a\xbc\x9a\x28\xa4\x00\x4e\x28\xef\x45\x14\x80\x50\x29\
    \x68\xa2\x80\x00\x78\xa2\x8a\x28\x01\x29\x28\xa2\x80\x0f\x43\x4b\xfc\x54\x51\x4c\
    \x07\x52\x76\xa2\x8a\x40\x21\xa4\xc9\xa2\x8a\x00\x71\xeb\x45\x14\x50\x03\x4f\x6a\
    \x31\xc8\xa2\x8a\x60\x1f\xe3\x4a\x4f\x02\x8a\x28\x00\xcf\x14\xb9\xe6\x8a\x29\x00\
    \x51\xd2\x8a\x28\x00\x3c\x52\x03\x45\x14\x01\xff\xd9";

    #[test]
    fn depacketize() {
        init_logging();
        let mut d = super::Depacketizer::new();
        let timestamp = crate::Timestamp {
            timestamp: 0,
            clock_rate: NonZeroU32::new(90_000).unwrap(),
            start: 0,
        };
        d.push(
            ReceivedPacketBuilder {
                ctx: crate::PacketContext::dummy(),
                stream_id: 0,
                timestamp,
                ssrc: 0,
                sequence_number: 0,
                loss: 0,
                mark: false,
                payload_type: 0,
            }
            .build(START_PACKET.iter().copied())
            .unwrap(),
        )
        .unwrap();
        assert!(d.pull().is_none());
        d.push(
            ReceivedPacketBuilder {
                ctx: crate::PacketContext::dummy(),
                stream_id: 0,
                timestamp,
                ssrc: 0,
                sequence_number: 1,
                loss: 0,
                mark: true,
                payload_type: 0,
            }
            .build(END_PACKET.iter().copied())
            .unwrap(),
        )
        .unwrap();

        let frame = match d.pull() {
            Some(CodecItem::VideoFrame(frame)) => frame,
            _ => panic!(),
        };
        assert_eq!(frame.data(), VALID_JPEG_IMAGE)
    }
}
