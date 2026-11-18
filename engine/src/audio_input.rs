//! Entrada de audio con cpal (Fase F2).
//! Abre un InputStream de-input, deposita muestras en un ring buffer lockfree
//! que el hilo RT drena en render(). Sin alloc en el callback: el Producer
//! se mueve al closure, el Consumer se guarda en el Renderer.
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::{HeapCons, HeapRb, traits::{Split, Producer}};
use cpal::traits::HostTrait;

/// Stream de entrada que se mantiene vivo.
pub struct AudioInputStream {
    #[allow(dead_code)]
    stream: cpal::Stream,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Abre el stream de entrada por defecto.
/// Devuelve (stream, consumer).
pub fn open(
    host: &cpal::Host,
    ring_capacity: usize,
) -> Result<(AudioInputStream, HeapCons<f32>), String> {
    let device = host.default_input_device().ok_or("sin dispositivo de entrada")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels();
    let sample_rate = config.sample_rate().0;
    let stream_config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let ring = HeapRb::new(ring_capacity);
    let (prod, cons) = ring.split();

    // El Producer se mueve al closure (es Send)
    let mut prod = prod;
    let stream = device
        .build_input_stream(
            &stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = prod.push_slice(data);
            },
            |err| eprintln!("[audio_input] error: {}", err),
            None,
        )
        .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;

    Ok((
        AudioInputStream {
            stream,
            channels,
            sample_rate,
        },
        cons,
    ))
}
