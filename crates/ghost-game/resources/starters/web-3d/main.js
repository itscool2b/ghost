// A 3D game with Three.js: the arrow keys move the player, Space jumps.
// Run `npm install` once, then `npm run dev`.
import * as THREE from "three";

const SPEED = 5;
const JUMP = 4.5;
const GRAVITY = 9.8;

const renderer = new THREE.WebGLRenderer({ antialias: true });
renderer.setPixelRatio(Math.min(2, window.devicePixelRatio));
renderer.setSize(window.innerWidth, window.innerHeight);
document.body.append(renderer.domElement);

const scene = new THREE.Scene();
scene.name = "World";
scene.background = new THREE.Color(0x14161c);

const camera = new THREE.PerspectiveCamera(60, window.innerWidth / window.innerHeight, 0.1, 100);
camera.position.set(0, 5, 8);
camera.lookAt(0, 0.5, 0);

const sun = new THREE.DirectionalLight(0xffffff, 2.2);
sun.name = "Sun";
sun.position.set(4, 8, 3);
scene.add(sun, new THREE.AmbientLight(0xffffff, 0.5));

const ground = new THREE.Mesh(
  new THREE.BoxGeometry(20, 0.2, 20),
  new THREE.MeshStandardMaterial({ color: 0x3a4152 }),
);
ground.name = "Ground";
ground.position.y = -0.1;
scene.add(ground);

const player = new THREE.Mesh(
  new THREE.CapsuleGeometry(0.4, 0.8, 8, 16),
  new THREE.MeshStandardMaterial({ color: 0x59c2ff }),
);
player.name = "Player";
player.position.y = 0.8;
scene.add(player);

// The whole game's state in one place, kept on the window so it can be read while it runs.
const game = { player, speedY: 0, onGround: true, keys: new Set() };
window.game = game;

addEventListener("keydown", (event) => {
  game.keys.add(event.code);
  if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space"].includes(event.code)) {
    event.preventDefault();
  }
});
addEventListener("keyup", (event) => game.keys.delete(event.code));
addEventListener("resize", () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
});

function update(delta) {
  const x = (game.keys.has("ArrowRight") ? 1 : 0) - (game.keys.has("ArrowLeft") ? 1 : 0);
  const z = (game.keys.has("ArrowDown") ? 1 : 0) - (game.keys.has("ArrowUp") ? 1 : 0);
  player.position.x = Math.max(-9.5, Math.min(9.5, player.position.x + x * SPEED * delta));
  player.position.z = Math.max(-9.5, Math.min(9.5, player.position.z + z * SPEED * delta));
  if (game.onGround && game.keys.has("Space")) {
    game.speedY = JUMP;
    game.onGround = false;
  }
  if (!game.onGround) {
    game.speedY -= GRAVITY * delta;
    player.position.y += game.speedY * delta;
    if (player.position.y <= 0.8) {
      player.position.y = 0.8;
      game.speedY = 0;
      game.onGround = true;
    }
  }
}

let last = performance.now();
function frame(now) {
  // A long gap (the tab was hidden) is played as one short step, not one long jump.
  const delta = Math.min(0.05, (now - last) / 1000);
  last = now;
  update(delta);
  renderer.render(scene, camera);
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
