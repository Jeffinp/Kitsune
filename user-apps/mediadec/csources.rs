// The C files of `mediadec` and how each group is compiled, relative to `user-apps/mediadec`.
// Shared (with `include!`) by this crate's build.rs (ring 3, freestanding clang) and the host checker
// `tools/media/mediacheck` (the host's clang), so both build exactly the same code.

/// libvpx v1.15.2: the VP9 decoder and the parts of vpx_dsp/vpx_scale/vpx_mem/vpx_util it uses
/// (generic C, plus the SSE2 intrinsics files listed in `VPX_SSE2`).
const VPX: &[&str] = &[
    "vp9/common/vp9_alloccommon.c",
    "vp9/common/vp9_blockd.c",
    "vp9/common/vp9_common_data.c",
    "vp9/common/vp9_entropy.c",
    "vp9/common/vp9_entropymode.c",
    "vp9/common/vp9_entropymv.c",
    "vp9/common/vp9_filter.c",
    "vp9/common/vp9_frame_buffers.c",
    "vp9/common/vp9_idct.c",
    "vp9/common/vp9_loopfilter.c",
    "vp9/common/vp9_pred_common.c",
    "vp9/common/vp9_quant_common.c",
    "vp9/common/vp9_reconinter.c",
    "vp9/common/vp9_reconintra.c",
    "vp9/common/vp9_rtcd.c",
    "vp9/common/vp9_scale.c",
    "vp9/common/vp9_scan.c",
    "vp9/common/vp9_seg_common.c",
    "vp9/common/vp9_thread_common.c",
    "vp9/common/vp9_tile_common.c",
    "vp9/decoder/vp9_decodeframe.c",
    "vp9/decoder/vp9_decodemv.c",
    "vp9/decoder/vp9_decoder.c",
    "vp9/decoder/vp9_detokenize.c",
    "vp9/decoder/vp9_dsubexp.c",
    "vp9/decoder/vp9_job_queue.c",
    "vp9/vp9_dx_iface.c",
    "vp9/vp9_iface_common.c",
    "vpx/src/vpx_codec.c",
    "vpx/src/vpx_decoder.c",
    "vpx_dsp/bitreader.c",
    "vpx_dsp/bitreader_buffer.c",
    "vpx_dsp/intrapred.c",
    "vpx_dsp/inv_txfm.c",
    "vpx_dsp/loopfilter.c",
    "vpx_dsp/prob.c",
    "vpx_dsp/vpx_convolve.c",
    "vpx_dsp/vpx_dsp_rtcd.c",
    "vpx_mem/vpx_mem.c",
    "vpx_scale/vpx_scale_rtcd.c",
    "vpx_scale/generic/yv12config.c",
    "vpx_scale/generic/yv12extend.c",
    "vpx_util/vpx_thread.c",
];

/// libvpx's SSE2 intrinsics (C, no assembly) that the hand-trimmed `csrc/vpx/*_rtcd.h` point to.
const VPX_SSE2: &[&str] = &[];

/// libopus 1.5.2, fixed-point decoder: CELT, SILK and the Opus layer.
const OPUS: &[&str] = &[
    "celt/bands.c",
    "celt/celt.c",
    "celt/celt_decoder.c",
    "celt/celt_lpc.c",
    "celt/cwrs.c",
    "celt/entcode.c",
    "celt/entdec.c",
    "celt/entenc.c",
    "celt/kiss_fft.c",
    "celt/laplace.c",
    "celt/mathops.c",
    "celt/mdct.c",
    "celt/modes.c",
    "celt/pitch.c",
    "celt/quant_bands.c",
    "celt/rate.c",
    "celt/vq.c",
    "silk/CNG.c",
    "silk/LPC_analysis_filter.c",
    "silk/LPC_fit.c",
    "silk/LPC_inv_pred_gain.c",
    "silk/NLSF2A.c",
    "silk/NLSF_decode.c",
    "silk/NLSF_stabilize.c",
    "silk/NLSF_unpack.c",
    "silk/PLC.c",
    "silk/bwexpander.c",
    "silk/bwexpander_32.c",
    "silk/code_signs.c",
    "silk/dec_API.c",
    "silk/decode_core.c",
    "silk/decode_frame.c",
    "silk/decode_indices.c",
    "silk/decode_parameters.c",
    "silk/decode_pitch.c",
    "silk/decode_pulses.c",
    "silk/decoder_set_fs.c",
    "silk/gain_quant.c",
    "silk/init_decoder.c",
    "silk/lin2log.c",
    "silk/log2lin.c",
    "silk/pitch_est_tables.c",
    "silk/resampler.c",
    "silk/resampler_private_AR2.c",
    "silk/resampler_private_IIR_FIR.c",
    "silk/resampler_private_down_FIR.c",
    "silk/resampler_private_up2_HQ.c",
    "silk/resampler_rom.c",
    "silk/shell_coder.c",
    "silk/sort.c",
    "silk/stereo_MS_to_LR.c",
    "silk/stereo_decode_pred.c",
    "silk/sum_sqr_shift.c",
    "silk/table_LSF_cos.c",
    "silk/tables_LTP.c",
    "silk/tables_NLSF_CB_NB_MB.c",
    "silk/tables_NLSF_CB_WB.c",
    "silk/tables_gain.c",
    "silk/tables_other.c",
    "silk/tables_pitch_lag.c",
    "silk/tables_pulses_per_block.c",
    "src/opus.c",
    "src/opus_decoder.c",
];

/// The project's own C: the decoder wrappers and the colour conversion.
const OWN: &[&str] = &["csrc/media.c", "csrc/yuv.c"];

/// Every C file with the extra flags of its group (include paths relative to `root`).
fn c_jobs(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<String>)> {
    let inc = |p: &str| format!("-I{}", root.join(p).display());
    let vpx_flags = vec![inc("vendor/libvpx"), inc("csrc/vpx"), "-w".to_string()];
    let opus_flags = vec![
        "-DHAVE_CONFIG_H".to_string(),
        inc("csrc/opus"),
        inc("vendor/opus/include"),
        inc("vendor/opus/celt"),
        inc("vendor/opus/silk"),
        inc("vendor/opus/silk/fixed"),
        "-w".to_string(),
    ];
    let own_flags = vec![
        inc("csrc"),
        inc("vendor/libvpx"),
        inc("csrc/vpx"),
        inc("vendor/opus/include"),
        "-Wall".to_string(),
    ];
    let mut jobs = Vec::new();
    for f in VPX.iter().chain(VPX_SSE2) {
        jobs.push((root.join("vendor/libvpx").join(f), vpx_flags.clone()));
    }
    for f in OPUS {
        jobs.push((root.join("vendor/opus").join(f), opus_flags.clone()));
    }
    for f in OWN {
        jobs.push((root.join(f), own_flags.clone()));
    }
    jobs
}
