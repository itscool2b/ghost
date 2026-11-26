/**
 * GLSL ES 3.00 sources for the orb. Written for Ghost; no third-party shader code.
 *
 * Look: soft oval light lobes orbit inside a disc, pushed around by a two-stage noise flow warp,
 * with faint ring ripples, mapped through the brand's four-stop colour ramp, fading to a soft
 * circular edge with a halo. Everything is procedural, so there are no texture files to load.
 *
 * Portability rules (WebKit/Safari 26 and Chromium both run this through ANGLE):
 * - `precision highp float` and `highp int`: the hash needs exact 32-bit unsigned maths, and WebGL2
 *   guarantees highp in fragment shaders.
 * - No extensions, derivatives, textures, or dynamic indexing; loops would need constant bounds, so
 *   there are none.
 * - GLSL ES has no implicit int/float conversion: every literal in float context carries a dot.
 *
 * `uTime` is a phase the CPU integrates as sum(dt * speed), so speed changes never make the
 * animation jump; the shader must not multiply it by `uSpeed` again. `uSpeed` instead stretches the
 * flow field so fast states streak. `uDrift` is likewise an offset the CPU integrates from the
 * brand's drift direction: multiplying a direction by the ever-growing phase in the shader would
 * fling the whole field sideways whenever the direction changed (switching agents).
 */

export const ORB_VERTEX_SHADER = /* glsl */ `#version 300 es
in vec2 position;
in vec2 uv;
out vec2 vUv;

void main() {
  vUv = uv;
  gl_Position = vec4(position, 0.0, 1.0);
}
`;

export const ORB_FRAGMENT_SHADER = /* glsl */ `#version 300 es
precision highp float;
precision highp int;

in vec2 vUv;
out vec4 fragColor;

uniform float uTime;
uniform vec2 uResolution;
uniform vec3 uPalette[4];
uniform float uEnergy;
uniform float uSpeed;
uniform float uTurbulence;
uniform float uRingFrequency;
uniform vec2 uDrift;
uniform float uSeed;

const float TAU = 6.283185307179586;
// Disc radius in orb units (1 = half the canvas). The rest of the canvas holds the halo.
const float ORB_RADIUS = 0.84;

// Integer avalanche: xor-shift and odd-multiply rounds. Exact on every GPU, unlike sin()-based
// hashes, whose quality collapses as their input grows (and uTime only grows).
uint avalanche(uint x) {
  x ^= x >> 15u;
  x *= 0x85ebca77u;
  x ^= x >> 13u;
  x *= 0xc2b2ae3du;
  x ^= x >> 16u;
  return x;
}

uint latticeHash(ivec3 cell, uint seed) {
  uvec3 u = uvec3(cell);
  return avalanche(u.x * 0x9e3779b1u ^ avalanche(u.y * 0x85ebca77u ^ avalanche(u.z * 0xc2b2ae3du ^ seed)));
}

// A pseudo-random gradient per lattice corner: three 10-bit fields of the hash, mapped to [-1, 1].
vec3 latticeGradient(ivec3 cell, uint seed) {
  uint h = latticeHash(cell, seed);
  return vec3(uvec3(h, h >> 10u, h >> 20u) & uvec3(1023u)) * (2.0 / 1023.0) - 1.0;
}

// 3D gradient noise with a quintic fade: smooth to the second derivative, so the flow warp never
// shows creases. Roughly in [-1, 1].
float gradientNoise(vec3 p, uint seed) {
  vec3 base = floor(p);
  vec3 f = p - base;
  ivec3 i = ivec3(base);
  vec3 w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);

  float n000 = dot(latticeGradient(i, seed), f);
  float n100 = dot(latticeGradient(i + ivec3(1, 0, 0), seed), f - vec3(1.0, 0.0, 0.0));
  float n010 = dot(latticeGradient(i + ivec3(0, 1, 0), seed), f - vec3(0.0, 1.0, 0.0));
  float n110 = dot(latticeGradient(i + ivec3(1, 1, 0), seed), f - vec3(1.0, 1.0, 0.0));
  float n001 = dot(latticeGradient(i + ivec3(0, 0, 1), seed), f - vec3(0.0, 0.0, 1.0));
  float n101 = dot(latticeGradient(i + ivec3(1, 0, 1), seed), f - vec3(1.0, 0.0, 1.0));
  float n011 = dot(latticeGradient(i + ivec3(0, 1, 1), seed), f - vec3(0.0, 1.0, 1.0));
  float n111 = dot(latticeGradient(i + ivec3(1, 1, 1), seed), f - vec3(1.0, 1.0, 1.0));

  return mix(
    mix(mix(n000, n100, w.x), mix(n010, n110, w.x), w.y),
    mix(mix(n001, n101, w.x), mix(n011, n111, w.x), w.y),
    w.z
  );
}

// Four-stop ramp, darkest to brightest, evenly spaced.
vec3 rampColor(float x) {
  float t = clamp(x, 0.0, 1.0) * 3.0;
  vec3 color = mix(uPalette[0], uPalette[1], clamp(t, 0.0, 1.0));
  color = mix(color, uPalette[2], clamp(t - 1.0, 0.0, 1.0));
  return mix(color, uPalette[3], clamp(t - 2.0, 0.0, 1.0));
}

// One soft oval of light on an orbit round the centre. Its long axis follows the orbit's tangent,
// so the lobes read as light sweeping round a sphere rather than blobs sliding past.
float lobe(vec2 q, float angle, float orbit, vec2 axes) {
  vec2 radial = vec2(cos(angle), sin(angle));
  vec2 d = q - radial * orbit;
  float along = dot(d, vec2(-radial.y, radial.x)) / axes.x;
  float across = dot(d, radial) / axes.y;
  return exp(-1.7 * (along * along + across * across));
}

void main() {
  vec2 res = max(uResolution, vec2(1.0));
  float minRes = min(res.x, res.y);
  vec2 p = (vUv * 2.0 - 1.0) * res / minRes;
  float pixel = 2.0 / minRes;
  float r = length(p);

  float energy = clamp(uEnergy, 0.0, 1.0);
  float t = uTime;
  uint seed = uint(clamp(uSeed, 0.0, 1.0) * 65521.0);
  vec2 seedShift = vec2(float(seed & 255u), float((seed >> 8u) & 255u)) * 0.37;
  float spinOffset = uSeed * TAU;

  // Flow warp, stage one: broad displacement drifting with the brand's direction. Fast states
  // stretch the field along y so motion reads as streaks.
  float stretch = 1.0 + 0.3 * clamp(uSpeed - 1.0, 0.0, 1.5);
  vec2 flow = (p - uDrift) * vec2(1.1, 1.1 * stretch) + seedShift;
  float turbulence = uTurbulence * (0.45 + 0.85 * energy);
  vec2 warp = vec2(
    gradientNoise(vec3(flow, t * 0.27), seed),
    gradientNoise(vec3(flow + 17.3, t * 0.27), seed)
  );
  vec2 q = p + warp * (0.3 * turbulence);

  // Stage two: a gentler, finer warp riding on the first; strong enough to keep the lobes organic,
  // weak enough not to tear them into streaks.
  vec2 detail = q * 2.3 + seedShift.yx - uDrift * 1.4;
  warp = vec2(
    gradientNoise(vec3(detail, t * 0.41 + 5.1), seed),
    gradientNoise(vec3(detail + 31.7, t * 0.41 + 5.1), seed)
  );
  q += warp * (0.06 * turbulence);

  // Three oval lobes on different orbits and spins.
  float spin = t * 0.23;
  float orbit = 0.2 + 0.16 * energy;
  vec2 axes = vec2(0.5 + 0.22 * energy, 0.27 + 0.08 * energy);
  float lobes = lobe(q, spin + spinOffset, orbit, axes);
  lobes += 0.85 * lobe(q, 2.1 - spin * 1.3 + spinOffset, orbit * 1.15, axes * vec2(0.9, 1.05));
  lobes += 0.7 * lobe(q, 4.2 + spin * 0.7 + spinOffset, orbit * 0.8, axes * vec2(1.1, 0.95));
  lobes *= 0.62;

  // The whole disc glows: an ambient floor keeps the gaps between lobes in the ramp's middle
  // instead of falling to its darkest stop, which read as holes.
  float light = 0.24 + 0.1 * energy;
  light += lobes * (0.75 + 0.35 * energy);
  light += (1.0 - smoothstep(0.0, ORB_RADIUS, r)) * (0.18 + 0.22 * energy);

  // Subtle ring ripples, broken up by noise. They fade out on small orbs, where they would only
  // alias, toward the rim, and as they get denser, so many rings read as texture, not contours.
  float ringDetail = smoothstep(28.0, 110.0, minRes) / (1.0 + 0.05 * uRingFrequency);
  float jitter = gradientNoise(vec3(q * 3.1 + seedShift, t * 0.55 + 11.0), seed);
  float rings = sin(length(q) * uRingFrequency * (0.5 * TAU) - t * 1.4 + jitter * 2.2);
  light += rings * (0.018 + 0.04 * energy) * ringDetail * (1.0 - 0.6 * smoothstep(0.4, ORB_RADIUS, r));

  // Light falls off toward the rim inside the ramp (deeper colour, never black), then a soft
  // shoulder rolls highlights off instead of clipping them.
  light *= 1.0 - 0.18 * smoothstep(0.45, ORB_RADIUS, r);
  float tone = 1.0 - exp(-max(light, 0.0) * 1.6);

  vec3 color = rampColor(tone);
  // A thin rim light gives the flat disc a glassy edge.
  color += uPalette[3] * smoothstep(ORB_RADIUS - 0.12, ORB_RADIUS - 0.02, r) * (0.04 + 0.05 * energy);

  // Soft circular edge (a feather of a few percent of the radius, never under two pixels), and a
  // faint halo beyond it that grows with energy.
  float edge = 1.0 - smoothstep(ORB_RADIUS - max(0.06, 2.0 * pixel), ORB_RADIUS + pixel, r);
  float halo = 1.0 - smoothstep(ORB_RADIUS, 1.0, r);
  halo *= halo * (0.1 + 0.22 * energy);
  float alpha = edge + halo * (1.0 - edge);
  vec3 rgb = mix(uPalette[2], color, edge);

  // Dither by half an 8-bit step so the dark end of the ramp never bands.
  uint grain = avalanche(uint(gl_FragCoord.x) * 0x9e3779b1u ^ avalanche(uint(gl_FragCoord.y) ^ seed));
  rgb = clamp(rgb + (float(grain & 1023u) / 1023.0 - 0.5) / 255.0, 0.0, 1.0);

  // Premultiplied alpha, matching the context's premultipliedAlpha: true.
  fragColor = vec4(rgb * alpha, alpha);
}
`;

/** Uniform names the renderer sets every frame, in the order it sets them. */
export const ORB_UNIFORMS = [
  "uTime",
  "uResolution",
  "uPalette",
  "uEnergy",
  "uSpeed",
  "uTurbulence",
  "uRingFrequency",
  "uDrift",
  "uSeed",
] as const;

export type OrbUniformName = (typeof ORB_UNIFORMS)[number];
