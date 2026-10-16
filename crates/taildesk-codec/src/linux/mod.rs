//! Backends de hardware do Linux: NVENC/NVDEC (NVIDIA, pelo driver proprietário) e VA-API (Intel,
//! AMD e Mesa). As bibliotecas são abertas em tempo de execução; faltando alguma, o backend some
//! da lista e a seleção cai para o próximo.

pub mod cuda;
pub mod gpu;
pub mod nvdec;
pub mod nvenc;
pub mod va;
pub mod vaapi;
#[rustfmt::skip]
pub mod sys {
    pub mod nvcuvid;
    pub mod nvenc;
    pub mod va;
    pub mod va_drm;
}
