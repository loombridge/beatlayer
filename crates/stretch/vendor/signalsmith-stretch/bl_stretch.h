// A C interface to Signalsmith Stretch, for Beatlayer's Complex and
// Complex Pro warp modes (the warp modes spec, R21 to R24).
//
// Signalsmith Stretch is a C++ library; this header and bl_stretch.cpp are
// Beatlayer's own small wrapper around it, so the Rust side needs no
// `bindgen` and no libclang. The vendor's own sources are beside this file,
// with their MIT licences (LICENSE.txt here, signalsmith-linear/LICENSE.txt
// there) and their provenance in README.md.
//
// The wrapper adds nothing to the library's behaviour: every call below is a
// straight pass-through. It exists only to fix an ABI the Rust side can call.

#ifndef BL_STRETCH_H
#define BL_STRETCH_H

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct BlStretch BlStretch;

// Configures a new stretcher for `channels` channels, in spectral blocks of
// `block_samples` at `interval_samples` apart.
BlStretch *bl_stretch_new(int channels, int block_samples, int interval_samples,
                          bool split_computation);
void bl_stretch_free(BlStretch *stretch);

int bl_stretch_block_samples(const BlStretch *stretch);
int bl_stretch_interval_samples(const BlStretch *stretch);
// The input and output latency, in frames.
int bl_stretch_input_latency(const BlStretch *stretch);
int bl_stretch_output_latency(const BlStretch *stretch);
void bl_stretch_reset(BlStretch *stretch);

// The pitch shift, as a frequency multiplier.
void bl_stretch_set_transpose_factor(BlStretch *stretch, double multiplier,
                                     double tonality_limit);
// The formant correction: `multiplier` scales the formants, and
// `compensate_pitch` keeps them in place through the pitch shift.
void bl_stretch_set_formant_factor(BlStretch *stretch, double multiplier,
                                   bool compensate_pitch);
// The rough fundamental the formant analysis assumes, normalised to the
// sample rate (a frequency in Hz divided by it).
void bl_stretch_set_formant_base(BlStretch *stretch, double base_freq);

// Primes the stretcher with the `input_samples` frames ending at the point
// playback is to start, without pre-calculating output (a resume).
void bl_stretch_seek(BlStretch *stretch, const float *const *inputs,
                     int input_samples, double playback_rate);
// Consumes `input_samples` input frames and writes `output_samples` output
// frames.
void bl_stretch_process(BlStretch *stretch, const float *const *inputs,
                        int input_samples, float *const *outputs,
                        int output_samples);
// Reads the tail of the output after the input has run out.
void bl_stretch_flush(BlStretch *stretch, float *const *outputs,
                      int output_samples);

#ifdef __cplusplus
}
#endif

#endif
