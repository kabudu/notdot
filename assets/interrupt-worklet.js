// Downmix and resample echo-cancelled microphone audio to 16 kHz mono in
// 100 ms chunks for notdot's on-device wake detector. Nothing is stored.
class Interrupt16k extends AudioWorkletProcessor {
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
    for (let i = 0; i < input[0].length; i++) {
      let sample = 0;
      for (const channel of input) sample += channel[i];
      sample /= input.length;
      while (this.position <= 1) {
        this.chunk[this.filled++] = this.previous + (sample - this.previous) * this.position;
        this.position += this.step;
        if (this.filled === this.chunk.length) {
          this.port.postMessage(this.chunk, [this.chunk.buffer]);
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
registerProcessor('interrupt-16k', Interrupt16k);
