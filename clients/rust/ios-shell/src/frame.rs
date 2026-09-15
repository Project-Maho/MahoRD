use maho_decode::Nv12Frame;

pub const FRAME_HEADER_SIZE: usize = 16;

pub fn repack_nv12_frame(frame: &Nv12Frame, sequence: u64) -> Vec<u8> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let uv_height = height.div_ceil(2);
    let y_size = width * height;
    let uv_size = width * uv_height;
    let total_size = FRAME_HEADER_SIZE + y_size + uv_size;

    let mut buf = vec![0u8; total_size];
    buf[0..4].copy_from_slice(&frame.width.to_le_bytes());
    buf[4..8].copy_from_slice(&frame.height.to_le_bytes());
    buf[8..16].copy_from_slice(&sequence.to_le_bytes());

    let y_dest = &mut buf[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + y_size];
    if frame.y_stride == width {
        let copy_len = y_size.min(frame.y_plane.len());
        y_dest[..copy_len].copy_from_slice(&frame.y_plane[..copy_len]);
    } else {
        for row in 0..height {
            let src_start = row * frame.y_stride;
            let src_end = (src_start + width).min(frame.y_plane.len());
            let dst_start = row * width;
            if src_start < frame.y_plane.len() {
                let to_copy = src_end - src_start;
                y_dest[dst_start..dst_start + to_copy]
                    .copy_from_slice(&frame.y_plane[src_start..src_end]);
            }
        }
    }

    let uv_dest = &mut buf[FRAME_HEADER_SIZE + y_size..FRAME_HEADER_SIZE + y_size + uv_size];
    if frame.uv_stride == width {
        let copy_len = uv_size.min(frame.uv_plane.len());
        uv_dest[..copy_len].copy_from_slice(&frame.uv_plane[..copy_len]);
    } else {
        for row in 0..uv_height {
            let src_start = row * frame.uv_stride;
            let src_end = (src_start + width).min(frame.uv_plane.len());
            let dst_start = row * width;
            if src_start < frame.uv_plane.len() {
                let to_copy = src_end - src_start;
                uv_dest[dst_start..dst_start + to_copy]
                    .copy_from_slice(&frame.uv_plane[src_start..src_end]);
            }
        }
    }

    buf
}
