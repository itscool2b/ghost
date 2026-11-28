/**
 * A 3D model of the project (a .glb), turned with the pointer. Loaded only when a model is
 * opened: three.js is a part of the app nobody else needs.
 */
import { useEffect, useRef, useState } from "react";
import {
  Box3,
  Color,
  DirectionalLight,
  HemisphereLight,
  PerspectiveCamera,
  Scene,
  Vector3,
  WebGLRenderer,
} from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

export default function ModelViewer({
  bytes,
  name,
}: {
  readonly bytes: Uint8Array;
  readonly name: string;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [state, setState] = useState<"loading" | "shown" | "failed">("loading");

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    let renderer: WebGLRenderer;
    try {
      renderer = new WebGLRenderer({ canvas: element, antialias: true, alpha: true });
    } catch {
      setState("failed");
      return;
    }
    const scene = new Scene();
    const camera = new PerspectiveCamera(40, 1, 0.01, 1000);
    scene.add(new HemisphereLight(new Color(0xffffff), new Color(0x444444), 2.2));
    const sun = new DirectionalLight(new Color(0xffffff), 2.4);
    sun.position.set(3, 5, 4);
    scene.add(sun);
    const controls = new OrbitControls(camera, element);
    controls.enablePan = false;
    let frame = 0;
    const draw = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        renderer.render(scene, camera);
      });
    };
    controls.addEventListener("change", draw);
    const fit = () => {
      const box = element.getBoundingClientRect();
      if (box.width === 0 || box.height === 0) return;
      renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
      renderer.setSize(box.width, box.height, false);
      camera.aspect = box.width / box.height;
      camera.updateProjectionMatrix();
      draw();
    };
    const watching = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(fit);
    watching?.observe(element);
    fit();

    let gone = false;
    // The app lets a picture load from a blob: address as a picture, not as a fetch. The loader
    // takes the first road when it finds no createImageBitmap, so it finds none while it is
    // made. The reader is put back before anything else runs.
    const reader = globalThis.createImageBitmap;
    try {
      (globalThis as { createImageBitmap?: unknown }).createImageBitmap = undefined;
      const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
      new GLTFLoader().parse(
        buffer as ArrayBuffer,
        "",
        (model) => {
          if (gone) return;
          scene.add(model.scene);
          const bounds = new Box3().setFromObject(model.scene);
          const size = bounds.getSize(new Vector3());
          const middle = bounds.getCenter(new Vector3());
          const reach = Math.max(size.x, size.y, size.z, 0.001);
          const distance = (reach / (2 * Math.tan((camera.fov * Math.PI) / 360))) * 1.6;
          camera.position.set(
            middle.x + distance * 0.6,
            middle.y + distance * 0.4,
            middle.z + distance,
          );
          camera.near = distance / 100;
          camera.far = distance * 100;
          camera.updateProjectionMatrix();
          controls.target.copy(middle);
          controls.update();
          setState("shown");
          draw();
          // Its pictures arrive after the shape: draw again as they do.
          const again = window.setInterval(draw, 200);
          window.setTimeout(() => window.clearInterval(again), 2000);
        },
        () => {
          if (!gone) setState("failed");
        },
      );
    } catch {
      setState("failed");
    } finally {
      globalThis.createImageBitmap = reader;
    }
    return () => {
      gone = true;
      if (frame) cancelAnimationFrame(frame);
      watching?.disconnect();
      controls.dispose();
      renderer.dispose();
    };
  }, [bytes]);

  return (
    <div className="game-asset-model" data-state={state} data-testid="game-asset-model">
      <canvas
        ref={canvas}
        aria-label={`${name}, a 3D model. Drag to turn it, scroll to come closer.`}
        role="img"
      />
      {state === "loading" ? (
        <p className="game-note" role="status">
          Reading the model
        </p>
      ) : null}
      {state === "failed" ? (
        <p className="game-note" role="status" data-testid="game-asset-model-failed">
          This model cannot be shown here. It may be packed in a way this viewer does not read
          (Draco, Meshopt or KTX2), or this computer has no 3D drawing for the window.
        </p>
      ) : null}
    </div>
  );
}
