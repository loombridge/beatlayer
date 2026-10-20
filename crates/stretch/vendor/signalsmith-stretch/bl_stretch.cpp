// Beatlayer's C wrapper around Signalsmith Stretch. See bl_stretch.h.

#include "bl_stretch.h"

#include <vector>

#include "signalsmith-stretch.h"

struct BlStretch {
  signalsmith::stretch::SignalsmithStretch<float> stretch;
  int channels;
  // The pointer arrays the library indexes, kept so a call allocates nothing
  // (the warp modes spec, R5).
  std::vector<float *> in_ptrs;
  std::vector<float *> out_ptrs;
};

namespace {

// The library takes `inputs[c][i]`; these copy a caller's const pointers into
// the wrapper's own arrays.
void gather_in(BlStretch *stretch, const float *const *inputs) {
  for (int c = 0; c < stretch->channels; ++c) {
    stretch->in_ptrs[c] = const_cast<float *>(inputs[c]);
  }
}

void gather_out(BlStretch *stretch, float *const *outputs) {
  for (int c = 0; c < stretch->channels; ++c) {
    stretch->out_ptrs[c] = outputs[c];
  }
}

}  // namespace

extern "C" {

BlStretch *bl_stretch_new(int channels, int block_samples, int interval_samples,
                          bool split_computation) {
  BlStretch *stretch = new BlStretch();
  stretch->channels = channels;
  stretch->stretch.configure(channels, block_samples, interval_samples,
                             split_computation);
  stretch->in_ptrs.resize(static_cast<size_t>(channels));
  stretch->out_ptrs.resize(static_cast<size_t>(channels));
  return stretch;
}

void bl_stretch_free(BlStretch *stretch) { delete stretch; }

int bl_stretch_block_samples(const BlStretch *stretch) {
  return stretch->stretch.blockSamples();
}

int bl_stretch_interval_samples(const BlStretch *stretch) {
  return stretch->stretch.intervalSamples();
}

int bl_stretch_input_latency(const BlStretch *stretch) {
  return stretch->stretch.inputLatency();
}

int bl_stretch_output_latency(const BlStretch *stretch) {
  return stretch->stretch.outputLatency();
}

void bl_stretch_reset(BlStretch *stretch) { stretch->stretch.reset(); }

void bl_stretch_set_transpose_factor(BlStretch *stretch, double multiplier,
                                     double tonality_limit) {
  stretch->stretch.setTransposeFactor(static_cast<float>(multiplier),
                                      static_cast<float>(tonality_limit));
}

void bl_stretch_set_formant_factor(BlStretch *stretch, double multiplier,
                                   bool compensate_pitch) {
  stretch->stretch.setFormantFactor(static_cast<float>(multiplier),
                                    compensate_pitch);
}

void bl_stretch_set_formant_base(BlStretch *stretch, double base_freq) {
  stretch->stretch.setFormantBase(static_cast<float>(base_freq));
}

void bl_stretch_seek(BlStretch *stretch, const float *const *inputs,
                     int input_samples, double playback_rate) {
  gather_in(stretch, inputs);
  stretch->stretch.seek(stretch->in_ptrs, input_samples, playback_rate);
}

void bl_stretch_process(BlStretch *stretch, const float *const *inputs,
                        int input_samples, float *const *outputs,
                        int output_samples) {
  gather_in(stretch, inputs);
  gather_out(stretch, outputs);
  stretch->stretch.process(stretch->in_ptrs, input_samples, stretch->out_ptrs,
                           output_samples);
}

void bl_stretch_flush(BlStretch *stretch, float *const *outputs,
                      int output_samples) {
  gather_out(stretch, outputs);
  stretch->stretch.flush(stretch->out_ptrs, output_samples);
}

}  // extern "C"
