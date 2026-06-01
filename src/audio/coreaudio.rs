use crate::error::{Error, Result};
use crate::pcm::Reader;
use std::os::raw::c_void;
use std::ptr;

#[repr(C)]
struct AudioStreamBasicDescription {
    m_sample_rate: f64,
    m_format_id: u32,
    m_format_flags: u32,
    m_bytes_per_packet: u32,
    m_frames_per_packet: u32,
    m_bytes_per_frame: u32,
    m_channels_per_frame: u32,
    m_bits_per_channel: u32,
    m_reserved: u32,
}

#[repr(C)]
struct AudioQueueBuffer {
    m_audio_data_bytes_capacity: u32,
    m_audio_data: *mut c_void,
    m_audio_data_byte_size: u32,
    m_user_data: *mut c_void,
    m_packet_description_capacity: u32,
    m_packet_descriptions: *mut c_void,
    m_packet_description_count: u32,
}

type AudioQueueRef = *mut c_void;
type AudioQueueBufferRef = *mut AudioQueueBuffer;
type OsStatus = i32;

type AudioQueueOutputCallback = extern "C" fn(
    in_user_data: *mut c_void,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
);

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewOutput(
        in_format: *const AudioStreamBasicDescription,
        in_callback_proc: AudioQueueOutputCallback,
        in_user_data: *mut c_void,
        in_callback_run_loop: *mut c_void,
        in_callback_run_loop_mode: *mut c_void,
        in_flags: u32,
        out_aq: *mut AudioQueueRef,
    ) -> OsStatus;

    fn AudioQueueAllocateBuffer(
        in_aq: AudioQueueRef,
        in_buffer_byte_size: u32,
        out_buffer: *mut AudioQueueBufferRef,
    ) -> OsStatus;

    fn AudioQueueEnqueueBuffer(
        in_aq: AudioQueueRef,
        in_buffer: AudioQueueBufferRef,
        in_num_packet_descs: u32,
        in_packet_descs: *const c_void,
    ) -> OsStatus;

    fn AudioQueueStart(in_aq: AudioQueueRef, in_device_start_time: *const c_void) -> OsStatus;

    fn AudioQueueStop(in_aq: AudioQueueRef, in_immediate: u8) -> OsStatus;
    fn AudioQueueFlush(in_aq: AudioQueueRef) -> OsStatus;

    fn AudioQueueDispose(in_aq: AudioQueueRef, in_immediate: u8) -> OsStatus;

    fn AudioQueueFreeBuffer(in_aq: AudioQueueRef, in_buffer: AudioQueueBufferRef) -> OsStatus;
}

const FORMAT_LPCM: u32 = 0x6C70_636D;
const FORMAT_FLAGS_FLOAT_PACKED: u32 = 0x9;
const SAMPLE_RATE: f64 = 48000.0;
const CHANNELS: u32 = 2;
const BITS_PER_CHANNEL: u32 = 32;
const BYTES_PER_FRAME: u32 = 8;
const FRAMES_PER_PACKET: u32 = 1;
const BYTES_PER_PACKET: u32 = 8;

const BUFFER_COUNT: usize = 3;
const FRAMES_PER_BUFFER: u32 = 4096;
const BUFFER_BYTES: u32 = FRAMES_PER_BUFFER * BYTES_PER_FRAME;

fn asbd() -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        m_sample_rate: SAMPLE_RATE,
        m_format_id: FORMAT_LPCM,
        m_format_flags: FORMAT_FLAGS_FLOAT_PACKED,
        m_bytes_per_packet: BYTES_PER_PACKET,
        m_frames_per_packet: FRAMES_PER_PACKET,
        m_bytes_per_frame: BYTES_PER_FRAME,
        m_channels_per_frame: CHANNELS,
        m_bits_per_channel: BITS_PER_CHANNEL,
        m_reserved: 0,
    }
}

fn fill_buffer(reader: &mut Reader, buffer: AudioQueueBufferRef) {
    unsafe {
        let buf = &mut *buffer;
        let capacity = buf.m_audio_data_bytes_capacity as usize;
        let sample_count = capacity / 4;
        let out = std::slice::from_raw_parts_mut(buf.m_audio_data as *mut f32, sample_count);
        let got = reader.pop(out);
        for slot in out.iter_mut().skip(got) {
            *slot = 0.0;
        }
        buf.m_audio_data_byte_size = capacity as u32;
    }
}

extern "C" fn render_callback(
    in_user_data: *mut c_void,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
) {
    unsafe {
        let reader = &mut *(in_user_data as *mut Reader);
        fill_buffer(reader, in_buffer);
        AudioQueueEnqueueBuffer(in_aq, in_buffer, 0, ptr::null());
    }
}

pub struct CoreAudioSink {
    reader: Box<Reader>,
    queue: AudioQueueRef,
    buffers: [AudioQueueBufferRef; BUFFER_COUNT],
    running: bool,
}

impl CoreAudioSink {
    pub fn new(reader: Reader) -> Self {
        CoreAudioSink {
            reader: Box::new(reader),
            queue: ptr::null_mut(),
            buffers: [ptr::null_mut(); BUFFER_COUNT],
            running: false,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        let format = asbd();
        let user_data = &mut *self.reader as *mut Reader as *mut c_void;
        let mut queue: AudioQueueRef = ptr::null_mut();

        unsafe {
            let status = AudioQueueNewOutput(
                &format,
                render_callback,
                user_data,
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut queue,
            );
            if status != 0 {
                return Err(Error::Audio(format!("AudioQueueNewOutput failed: {status}")));
            }
            self.queue = queue;

            for slot in self.buffers.iter_mut() {
                let mut buffer: AudioQueueBufferRef = ptr::null_mut();
                let status = AudioQueueAllocateBuffer(queue, BUFFER_BYTES, &mut buffer);
                if status != 0 {
                    return Err(Error::Audio(format!(
                        "AudioQueueAllocateBuffer failed: {status}"
                    )));
                }
                fill_buffer(&mut self.reader, buffer);
                let status = AudioQueueEnqueueBuffer(queue, buffer, 0, ptr::null());
                if status != 0 {
                    return Err(Error::Audio(format!(
                        "AudioQueueEnqueueBuffer failed: {status}"
                    )));
                }
                *slot = buffer;
            }

            let status = AudioQueueStart(queue, ptr::null());
            if status != 0 {
                return Err(Error::Audio(format!("AudioQueueStart failed: {status}")));
            }
        }

        self.running = true;
        Ok(())
    }

    pub fn flush(&self) {
        if !self.queue.is_null() {
            unsafe {
                AudioQueueFlush(self.queue);
            }
        }
    }

    pub fn stop(&mut self) {
        if self.queue.is_null() {
            return;
        }
        unsafe {
            if self.running {
                AudioQueueStop(self.queue, 1);
                self.running = false;
            }
            for slot in self.buffers.iter_mut() {
                if !slot.is_null() {
                    AudioQueueFreeBuffer(self.queue, *slot);
                    *slot = ptr::null_mut();
                }
            }
            AudioQueueDispose(self.queue, 1);
            self.queue = ptr::null_mut();
        }
    }
}

impl Drop for CoreAudioSink {
    fn drop(&mut self) {
        self.stop();
    }
}
