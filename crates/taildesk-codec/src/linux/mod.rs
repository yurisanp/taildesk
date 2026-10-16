//! Backends de hardware só do Linux: VA-API (Intel, AMD e Mesa) e o inventário de GPUs pelo
//! sysfs. As bibliotecas são abertas em tempo de execução; faltando alguma, o backend some da
//! lista e a seleção cai para o próximo. NVENC/NVDEC ficam em [`crate::nvidia`].

pub mod gpu;
pub mod va;
pub mod vaapi;
#[rustfmt::skip]
pub mod sys {
    pub mod va;
    pub mod va_drm;
}
