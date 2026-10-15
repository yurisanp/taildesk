use super::*;

#[test]
fn cores_conhecidas() {
    use Faixa::*;
    use Matriz::*;
    assert_eq!(
        yuv_para_rgb(235, 128, 128, Bt709, Limitada),
        [255, 255, 255]
    );
    assert_eq!(yuv_para_rgb(16, 128, 128, Bt709, Limitada), [0, 0, 0]);
    assert_eq!(yuv_para_rgb(255, 128, 128, Bt601, Cheia), [255, 255, 255]);
    // Vermelho 100% em BT.709 e BT.601, faixa limitada.
    let perto = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2);
    assert!(perto(
        yuv_para_rgb(63, 102, 240, Bt709, Limitada),
        [255, 0, 0]
    ));
    assert!(perto(
        yuv_para_rgb(81, 90, 240, Bt601, Limitada),
        [255, 0, 0]
    ));
    // Azul 100% em BT.709.
    assert!(perto(
        yuv_para_rgb(32, 240, 118, Bt709, Limitada),
        [0, 0, 255]
    ));
}

/// Device de verdade, se houver (no CI pode não haver GPU nem renderizador por software).
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Lê a textura de saída para a CPU (linhas alinhadas em 256 bytes).
fn ler(device: &wgpu::Device, queue: &wgpu::Queue, t: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (l, a) = (t.width(), t.height());
    let passo = (l * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(passo * a),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut cmd = device.create_command_encoder(&Default::default());
    cmd.copy_texture_to_buffer(
        t.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(passo),
                rows_per_image: Some(a),
            },
        },
        t.size(),
    );
    queue.submit([cmd.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let dados = buf.slice(..).get_mapped_range().unwrap();
    let mut out = Vec::new();
    for y in 0..a {
        for x in 0..l {
            let i = (y * passo + x * 4) as usize;
            out.push([dados[i], dados[i + 1], dados[i + 2], dados[i + 3]]);
        }
    }
    out
}

/// Quadro 64×32 em blocos de 16×16 com cores diferentes; compara o centro de cada bloco com a
/// conversão na CPU. Passo maior que a largura, como sai dos decoders.
#[test]
fn nv12_e_i420_na_gpu() {
    let Some((device, queue)) = device() else {
        eprintln!("sem adaptador wgpu: teste da GPU pulado");
        return;
    };
    let conv = Conversor::new(&device);
    let (l, a, passo) = (64u32, 32u32, 80u32);
    let bloco = |x: u32, y: u32| -> (u8, u8, u8) {
        let b = (x / 16 + 4 * (y / 16)) as u8;
        (20 + b * 25, 40 + b * 20, 220 - b * 18)
    };
    let mut y_plano = vec![0u8; (passo * a) as usize];
    let mut uv = vec![0u8; (passo * a / 2) as usize];
    let mut u = vec![0u8; (passo / 2 * a / 2) as usize];
    let mut v = vec![0u8; (passo / 2 * a / 2) as usize];
    for py in 0..a {
        for px in 0..l {
            y_plano[(py * passo + px) as usize] = bloco(px, py).0;
        }
    }
    for cy in 0..a / 2 {
        for cx in 0..l / 2 {
            let (_, cb, cr) = bloco(cx * 2, cy * 2);
            uv[(cy * passo + cx * 2) as usize] = cb;
            uv[(cy * passo + cx * 2 + 1) as usize] = cr;
            u[(cy * passo / 2 + cx) as usize] = cb;
            v[(cy * passo / 2 + cx) as usize] = cr;
        }
    }
    let casos: [(Formato, Vec<Plano<'_>>); 2] = [
        (
            Formato::Nv12,
            vec![
                Plano {
                    dados: &y_plano,
                    passo,
                },
                Plano { dados: &uv, passo },
            ],
        ),
        (
            Formato::I420,
            vec![
                Plano {
                    dados: &y_plano,
                    passo,
                },
                Plano {
                    dados: &u,
                    passo: passo / 2,
                },
                Plano {
                    dados: &v,
                    passo: passo / 2,
                },
            ],
        ),
    ];
    for (formato, planos) in casos {
        let s = Superficie::new(
            &device,
            &conv,
            formato,
            l,
            a,
            Matriz::Bt709,
            Faixa::Limitada,
        );
        s.quadro(&queue, &device, &conv, &planos).unwrap();
        let px = ler(&device, &queue, s.textura());
        for by in 0..a / 16 {
            for bx in 0..l / 16 {
                let (x, y) = (bx * 16 + 8, by * 16 + 8);
                let (yy, cb, cr) = bloco(x, y);
                let esperado = yuv_para_rgb(yy, cb, cr, Matriz::Bt709, Faixa::Limitada);
                let lido = px[(y * l + x) as usize];
                for c in 0..3 {
                    assert!(
                        lido[c].abs_diff(esperado[c]) <= 2,
                        "{formato:?} bloco ({bx},{by}): GPU {lido:?}, CPU {esperado:?}"
                    );
                }
                assert_eq!(lido[3], 255);
            }
        }
    }
}

#[test]
fn quadro_com_plano_curto_e_recusado() {
    let Some((device, queue)) = device() else {
        return;
    };
    let conv = Conversor::new(&device);
    let s = Superficie::new(
        &device,
        &conv,
        Formato::Nv12,
        16,
        16,
        Matriz::Bt601,
        Faixa::Limitada,
    );
    let y = [0u8; 16 * 16];
    let uv = [0u8; 16 * 8 - 1];
    let planos = [
        Plano {
            dados: &y,
            passo: 16,
        },
        Plano {
            dados: &uv,
            passo: 16,
        },
    ];
    assert_eq!(
        s.quadro(&queue, &device, &conv, &planos),
        Err(ErroQuadro::Tamanho)
    );
    assert_eq!(
        s.quadro(&queue, &device, &conv, &planos[..1]),
        Err(ErroQuadro::Planos)
    );
}
