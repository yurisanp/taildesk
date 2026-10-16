//! NVENC e NVDEC (GPUs NVIDIA), no Linux e no Windows, pelas bibliotecas do driver abertas em
//! tempo de execução (`libcuda.so.1`/`nvcuda.dll`, `libnvidia-encode.so.1`/`nvEncodeAPI64.dll`,
//! `libnvcuvid.so.1`/`nvcuvid.dll`). Sem driver da NVIDIA os backends somem da lista.
//!
//! As bindings foram geradas no Linux x86_64; os tipos `c_ulong` do cuviddec.h seguem o
//! tamanho do `unsigned long` de cada sistema, então valem também no Windows x64.

pub mod cuda;
pub mod nvdec;
pub mod nvenc;
#[rustfmt::skip]
pub mod sys {
    pub mod nvcuvid;
    pub mod nvenc;
}

/// Nome da biblioteca do driver neste sistema.
pub(crate) const fn biblioteca(linux: &'static str, windows: &'static str) -> &'static str {
    if cfg!(windows) { windows } else { linux }
}
