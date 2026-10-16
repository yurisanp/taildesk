//! SPS e PPS do H.264 (ITU-T H.264 §7.3.2) para os encoders que pedem os cabeçalhos prontos
//! (VA-API com packed headers). Sempre progressivo, 4:2:0 de 8 bits, POC tipo 2, uma
//! referência e sem reordenação, com o VUI dizendo BT.709 de faixa limitada.

/// Escritor de bits MSB primeiro, com Exp-Golomb.
#[derive(Debug, Default)]
struct Bits {
    bytes: Vec<u8>,
    atual: u8,
    n: u8,
}

impl Bits {
    fn u(&mut self, bits: u8, valor: u32) {
        for i in (0..bits).rev() {
            self.atual = (self.atual << 1) | ((valor >> i) & 1) as u8;
            self.n += 1;
            if self.n == 8 {
                self.bytes.push(self.atual);
                (self.atual, self.n) = (0, 0);
            }
        }
    }

    fn flag(&mut self, f: bool) {
        self.u(1, u32::from(f));
    }

    fn ue(&mut self, v: u32) {
        let v = u64::from(v) + 1;
        let tamanho = 64 - v.leading_zeros() as u8;
        self.u(tamanho - 1, 0);
        for i in (0..tamanho).rev() {
            self.u(1, ((v >> i) & 1) as u32);
        }
    }

    fn se(&mut self, v: i32) {
        self.ue(if v > 0 {
            2 * v as u32 - 1
        } else {
            2 * v.unsigned_abs()
        });
    }

    /// `rbsp_trailing_bits` e os bytes do RBSP.
    fn fim(mut self) -> Vec<u8> {
        self.u(1, 1);
        while self.n != 0 {
            self.u(1, 0);
        }
        self.bytes
    }
}

/// NAL em Annex-B (start code de 4 bytes) com os bytes de prevenção de emulação.
fn nal(ref_idc: u8, tipo: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, (ref_idc << 5) | tipo];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros == 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Perfil do stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perfil {
    ConstrainedBaseline,
    Main,
    High,
}

impl Perfil {
    pub fn cabac(self) -> bool {
        self != Perfil::ConstrainedBaseline
    }

    pub fn transformada_8x8(self) -> bool {
        self == Perfil::High
    }
}

/// O que o SPS e o PPS descrevem.
#[derive(Clone, Copy, Debug)]
pub struct Parametros {
    pub perfil: Perfil,
    pub level_idc: u8,
    pub largura: u32,
    pub altura: u32,
    pub log2_max_frame_num_menos4: u32,
    pub fps: u32,
}

impl Parametros {
    pub fn macroblocos(&self) -> (u32, u32) {
        (self.largura.div_ceil(16), self.altura.div_ceil(16))
    }
}

/// SPS em Annex-B.
pub fn sps(p: &Parametros) -> Vec<u8> {
    let mut b = Bits::default();
    let (profile_idc, constraint) = match p.perfil {
        // constraint_set0 e constraint_set1: Constrained Baseline.
        Perfil::ConstrainedBaseline => (66, 0b1100_0000),
        Perfil::Main => (77, 0b0100_0000),
        Perfil::High => (100, 0),
    };
    b.u(8, profile_idc);
    b.u(8, constraint);
    b.u(8, u32::from(p.level_idc));
    b.ue(0); // seq_parameter_set_id
    if p.perfil == Perfil::High {
        b.ue(1); // chroma_format_idc: 4:2:0
        b.ue(0); // bit_depth_luma_minus8
        b.ue(0); // bit_depth_chroma_minus8
        b.flag(false); // qpprime_y_zero_transform_bypass_flag
        b.flag(false); // seq_scaling_matrix_present_flag
    }
    b.ue(p.log2_max_frame_num_menos4);
    b.ue(2); // pic_order_cnt_type
    b.ue(1); // max_num_ref_frames
    b.flag(false); // gaps_in_frame_num_value_allowed_flag
    let (mbs_l, mbs_a) = p.macroblocos();
    b.ue(mbs_l - 1);
    b.ue(mbs_a - 1);
    b.flag(true); // frame_mbs_only_flag
    b.flag(true); // direct_8x8_inference_flag
    let (corte_d, corte_b) = ((mbs_l * 16 - p.largura) / 2, (mbs_a * 16 - p.altura) / 2);
    b.flag(corte_d != 0 || corte_b != 0);
    if corte_d != 0 || corte_b != 0 {
        b.ue(0);
        b.ue(corte_d);
        b.ue(0);
        b.ue(corte_b);
    }
    b.flag(true); // vui_parameters_present_flag
    b.flag(false); // aspect_ratio_info_present_flag
    b.flag(false); // overscan_info_present_flag
    b.flag(true); // video_signal_type_present_flag
    b.u(3, 5); // video_format: não especificado
    b.flag(false); // video_full_range_flag
    b.flag(true); // colour_description_present_flag
    b.u(8, 1); // colour_primaries: BT.709
    b.u(8, 1); // transfer_characteristics: BT.709
    b.u(8, 1); // matrix_coefficients: BT.709
    b.flag(false); // chroma_loc_info_present_flag
    b.flag(true); // timing_info_present_flag
    b.u(32, 1); // num_units_in_tick
    b.u(32, 2 * p.fps.max(1)); // time_scale
    b.flag(false); // fixed_frame_rate_flag: a captura entrega quando a tela muda
    b.flag(false); // nal_hrd_parameters_present_flag
    b.flag(false); // vcl_hrd_parameters_present_flag
    b.flag(false); // pic_struct_present_flag
    b.flag(true); // bitstream_restriction_flag
    b.flag(true); // motion_vectors_over_pic_boundaries_flag
    b.ue(0); // max_bytes_per_pic_denom
    b.ue(0); // max_bits_per_mb_denom
    b.ue(15); // log2_max_mv_length_horizontal
    b.ue(15); // log2_max_mv_length_vertical
    b.ue(0); // max_num_reorder_frames: o decoder entrega cada quadro na hora
    b.ue(1); // max_dec_frame_buffering
    nal(3, 7, &b.fim())
}

/// PPS em Annex-B.
pub fn pps(p: &Parametros) -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.flag(p.perfil.cabac()); // entropy_coding_mode_flag
    b.flag(false); // bottom_field_pic_order_in_frame_present_flag
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.flag(false); // weighted_pred_flag
    b.u(2, 0); // weighted_bipred_idc
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.flag(true); // deblocking_filter_control_present_flag
    b.flag(false); // constrained_intra_pred_flag
    b.flag(false); // redundant_pic_cnt_present_flag
    if p.perfil == Perfil::High {
        b.flag(p.perfil.transformada_8x8()); // transform_8x8_mode_flag
        b.flag(false); // pic_scaling_matrix_present_flag
        b.se(0); // second_chroma_qp_index_offset
    }
    nal(3, 8, &b.fim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        let mut b = Bits::default();
        for v in [0, 1, 2, 3, 7] {
            b.ue(v);
        }
        b.se(-1);
        // 1 010 011 00100 0001000 011 → bytes com o trailing.
        assert_eq!(b.fim(), [0b1010_0110, 0b0100_0001, 0b0000_1110]);
    }

    #[test]
    fn prevencao_de_emulacao() {
        assert_eq!(
            nal(3, 7, &[0, 0, 1, 0, 0, 0, 5]),
            [0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0, 5]
        );
    }

    #[test]
    fn sps_conhecido_1080p() {
        let p = Parametros {
            perfil: Perfil::High,
            level_idc: 41,
            largura: 1920,
            altura: 1080,
            log2_max_frame_num_menos4: 4,
            fps: 60,
        };
        let s = sps(&p);
        // profile_idc 100, constraints 0, level 4.1.
        assert_eq!(&s[..8], &[0, 0, 0, 1, 0x67, 100, 0, 41]);
        assert_eq!(&pps(&p)[..5], &[0, 0, 0, 1, 0x68]);
    }
}
