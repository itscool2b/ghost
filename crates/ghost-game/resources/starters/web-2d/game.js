// A 2D game on a plain canvas: the left and right arrows move, Space or Enter jumps.
// The whole game is in `game`, kept on the window so it can be read while it runs.
const canvas = document.getElementById("game");
const context = canvas.getContext("2d");

const SPEED = 300;
const JUMP = -520;
const GRAVITY = 1400;
const GROUND = 480;

const game = {
  player: { x: 200, y: GROUND - 48, width: 32, height: 48, speedY: 0, onGround: true },
  keys: new Set(),
};
window.game = game;

addEventListener("keydown", (event) => {
  game.keys.add(event.code);
  if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Space"].includes(event.code)) {
    event.preventDefault();
  }
});
addEventListener("keyup", (event) => game.keys.delete(event.code));

function update(delta) {
  const player = game.player;
  const direction = (game.keys.has("ArrowRight") ? 1 : 0) - (game.keys.has("ArrowLeft") ? 1 : 0);
  player.x = Math.max(
    0,
    Math.min(canvas.width - player.width, player.x + direction * SPEED * delta),
  );
  if (player.onGround && (game.keys.has("Space") || game.keys.has("Enter"))) {
    player.speedY = JUMP;
    player.onGround = false;
  }
  if (!player.onGround) {
    player.speedY += GRAVITY * delta;
    player.y += player.speedY * delta;
    if (player.y >= GROUND - player.height) {
      player.y = GROUND - player.height;
      player.speedY = 0;
      player.onGround = true;
    }
  }
}

function draw() {
  context.fillStyle = "#171a21";
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.fillStyle = "#3a4152";
  context.fillRect(0, GROUND, canvas.width, canvas.height - GROUND);
  context.fillStyle = "#59c2ff";
  context.fillRect(game.player.x, game.player.y, game.player.width, game.player.height);
}

let last = performance.now();
function frame(now) {
  // A long gap (the tab was hidden) is played as one short step, not one long jump.
  const delta = Math.min(0.05, (now - last) / 1000);
  last = now;
  update(delta);
  draw();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
