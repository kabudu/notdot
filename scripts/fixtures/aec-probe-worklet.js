// Downmix and resample microphone audio to 16 kHz mono for the local wake
// detector. Posts 100 ms chunks with their RMS. Test fixture only.
class Probe16k extends AudioWorkletProcessor {
  constructor() {
    super();
    this.step = sampleRate / 16000;
    this.position = 0;
    this.previous = 0;
    this.chunk = new Float32Array(1600);
    this.filled = 0;
  }
  process(inputs) {
    const input = inputs[0];
    if (!input || !input[0]) return true;
    const frames = input[0].length;
    for (let i = 0; i < frames; i++) {
      let sample = 0;
      for (const channel of input) sample += channel[i];
      sample /= input.length;
      // Linear interpolation between the previous and current input sample.
      while (this.position <= 1) {
        this.chunk[this.filled++] = this.previous + (sample - this.previous) * this.position;
        this.position += this.step;
        if (this.filled === this.chunk.length) {
          let energy = 0;
          for (const s of this.chunk) energy += s * s;
          this.port.postMessage({ samples: this.chunk, rms: Math.sqrt(energy / this.chunk.length) });
          this.chunk = new Float32Array(1600);
          this.filled = 0;
        }
      }
      this.position -= 1;
      this.previous = sample;
    }
    return true;
  }
}
registerProcessor('probe-16k', Probe16k);
