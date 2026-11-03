// Beatlayer's own shim over Link Audio's sink (the Link Audio spec, R11). Link's C interface allocates a buffer handle with `new` in
// `abl_link_audio_sink_retain_buffer` and frees it in the commit, which the
// audio thread may not do. The C++ handle holds no heap of its own, so this
// keeps it on the stack: retain, fill, commit and release in one call.

#include <abl_link.h>
#include <ableton/LinkAudio.hpp>

#include <algorithm>
#include <cmath>

extern "C" bool bl_link_audio_sink_send(struct abl_link_audio_sink sink,
  abl_link_session_state session_state,
  const float *left,
  const float *right,
  size_t frames,
  double beats_at_buffer_begin,
  double quantum,
  uint32_t sample_rate)
{
  auto *cppSink = reinterpret_cast<ableton::LinkAudioSink *>(sink.impl);
  const auto *state = reinterpret_cast<ableton::Link::SessionState *>(session_state.impl);
  ableton::LinkAudioSink::BufferHandle handle(*cppSink);
  if (!handle || handle.maxNumSamples < frames * 2)
  {
    // The handle's destructor releases a buffer it holds.
    return false;
  }
  const auto to16 = [](float x)
  { return static_cast<int16_t>(std::lrintf(std::clamp(x, -1.0f, 1.0f) * 32767.0f)); };
  for (size_t i = 0; i < frames; ++i)
  {
    handle.samples[2 * i] = to16(left[i]);
    handle.samples[2 * i + 1] = to16(right[i]);
  }
  return handle.commit(*state, beats_at_buffer_begin, quantum, frames, 2, sample_rate);
}
