# Ghost's bridge inside a Godot game it runs (docs/game.md). It connects out to Ghost on this
# computer, proves itself with the run's token, and from then on does what Ghost asks: sends the
# frames the pane shows, takes input, freezes and steps the game, and answers questions about the
# running scene. It never listens for connections, and when Ghost goes away the game quits.
#
# Messages are frames of [u32 length, little-endian][u8 kind][payload]; the length counts the kind
# byte. Kind 1 is JSON, 2 a JPEG frame for the pane, 3 a picture that answers a request.
extends Node

const KIND_JSON := 1
const KIND_FRAME := 2
const KIND_IMAGE := 3

## One step of game time: the game is started with `--fixed-fps 60`.
const STEP_USEC := 16667
const MAX_MESSAGE := 4 * 1024 * 1024
const MAX_OUTLINE := 500
const MAX_CHILDREN := 30
const MAX_PENDING_ERRORS := 200
## The most inputs a run's tape keeps. A pointer that never rests fills it in about five minutes;
## keys alone take hours.
const MAX_TAPE := 20000
## And the most code and values it keeps, in characters.
const MAX_TAPE_CODE := 1024 * 1024
## A small picture of the game is kept this often, in steps, and this many are kept: the last
## fifteen seconds.
const RECENT_EVERY := 30
const RECENT_KEEP := 31

var _peer := StreamPeerTCP.new()
var _greeted := false
var _inbox := PackedByteArray()

## Frozen by Ghost: the tree is paused, and the game's own pause is remembered underneath.
## `_frozen` is what Ghost asked for and `_stopped` what the tree is now. They meet after a
## frame is drawn and never in the middle of one, so every step the game takes is a whole one,
## its physics and then its nodes, and a replay can take the very same steps.
var _frozen := false
var _stopped := false
var _game_paused := false
## Steps of the game taken since it began: frames in which it ran. Frozen frames are not steps.
var _tick := 0
## This frame is a step.
var _running := false
## Everything done to the game from outside, by Ghost's pane or by an agent: every input, and
## every value set and piece of code run in it. Each is [step, phase, event]. Phase 1 was done
## while that step ran; phase 0 was done while the game stood still before it.
var _tape: Array = []
var _tape_full := false
## The code on the tape, in characters: a tape is kept small enough to hand back.
var _tape_code := 0
## A replay is giving the tape back: what it does is written by the replay itself.
var _replaying := false
## Small pictures of the last seconds: [step, Image].
var _recent: Array = []
var _speed := 1.0
var _deadline := 0

var _watching := false
var _watch_max := 960
var _watch_every := 2
var _sent := 0
var _acked := 0
var _frame := 0
var _since_stats := 0

## The action or wait under way: its steps, how far it is, and the pictures taken along it.
var _job: Dictionary = {}
var _held: Array = []
var _refs: Dictionary = {}
var _errors: Array = []
var _errors_mutex := Mutex.new()
var _logger: Object = null

func _ready() -> void:
	process_mode = Node.PROCESS_MODE_ALWAYS
	process_priority = -1000000
	AudioServer.set_bus_mute(0, true)
	var endpoint := OS.get_environment("GHOST_GAME_BRIDGE").split(":")
	if endpoint.size() != 2 or endpoint[0] != "127.0.0.1":
		push_error("Ghost: no bridge address was given to this game.")
		get_tree().quit(73)
		return
	_peer.connect_to_host(endpoint[0], int(endpoint[1]))
	RenderingServer.frame_post_draw.connect(_after_draw)
	_install_logger()
	# A run started to be replayed stands still from before its first step.
	if OS.get_environment("GHOST_GAME_HOLD") == "1":
		_frozen = true
		_settle_frozen()

func _install_logger() -> void:
	# Godot 4.5 and later hand errors to a Logger with their file and line. Its script is apart,
	# so an older Godot that has no Logger class still loads this bridge.
	if not ClassDB.class_exists("Logger"):
		return
	var folder := (get_script() as Script).resource_path.get_base_dir()
	var script: Script = load(folder.path_join("logger.gd"))
	if script == null:
		return
	_logger = script.new()
	_logger.set("bridge", self)
	OS.call("add_logger", _logger)

## Called by the logger, from any thread.
func report(entry: Dictionary) -> void:
	_errors_mutex.lock()
	if _errors.size() < MAX_PENDING_ERRORS:
		_errors.append(entry)
	_errors_mutex.unlock()

func _process(_delta: float) -> void:
	_frame += 1
	_running = not _stopped
	if _running:
		_tick += 1
	_pump()
	if not _job.is_empty():
		# An action moves on only in a step; one that waits for the game to thaw waits a frame.
		if _running:
			_advance_job()
		return
	if _stopped:
		OS.delay_msec(15)
		return
	# Live: the game runs at a fixed step with no clock of its own, so real time is kept here.
	var now := Time.get_ticks_usec()
	if _deadline == 0 or now - _deadline > 250000:
		_deadline = now
	_deadline += int(STEP_USEC / _speed)
	var wait := _deadline - Time.get_ticks_usec()
	if wait > 0:
		OS.delay_usec(wait)

# ---------- the connection ----------

func _pump() -> void:
	_peer.poll()
	var status := _peer.get_status()
	if status == StreamPeerTCP.STATUS_CONNECTING:
		return
	if status != StreamPeerTCP.STATUS_CONNECTED:
		# Ghost is gone, or never answered: a game nobody watches does not stay.
		get_tree().quit(0 if _greeted else 74)
		return
	if not _greeted:
		_greeted = true
		_peer.set_no_delay(true)
		var size := get_viewport().get_visible_rect().size
		_send_json({
			"hello": OS.get_environment("GHOST_GAME_TOKEN"),
			"engine": "godot",
			"version": str(Engine.get_version_info().get("string", "")),
			"width": int(size.x),
			"height": int(size.y),
			"pid": OS.get_process_id(),
			"window": DisplayServer.window_get_native_handle(DisplayServer.WINDOW_HANDLE),
			"project": str(ProjectSettings.get_setting("application/config/name", "")),
			"renderer": RenderingServer.get_current_rendering_driver_name(),
			"joypads": Input.get_connected_joypads().size(),
		})
	var available := _peer.get_available_bytes()
	if available > 0:
		var got := _peer.get_data(available)
		if got[0] == OK:
			_inbox.append_array(got[1])
	while _inbox.size() >= 5:
		var length := _inbox.decode_u32(0)
		if length < 1 or length > MAX_MESSAGE:
			get_tree().quit(75)
			return
		if _inbox.size() < 4 + length:
			break
		var kind := _inbox[4]
		var payload := _inbox.slice(5, 4 + length)
		_inbox = _inbox.slice(4 + length)
		if kind == KIND_JSON:
			var message = JSON.parse_string(payload.get_string_from_utf8())
			if message is Dictionary:
				_handle(message)
	_flush_errors()

func _send(kind: int, payload: PackedByteArray) -> void:
	var head := PackedByteArray()
	head.resize(5)
	head.encode_u32(0, payload.size() + 1)
	head[4] = kind
	_peer.put_data(head)
	_peer.put_data(payload)

func _send_json(message: Dictionary) -> void:
	_send(KIND_JSON, JSON.stringify(message).to_utf8_buffer())

func _reply(id, fields: Dictionary = {}) -> void:
	if id == null:
		return
	# What the request made go wrong is said before its answer, so Ghost has it when it reads
	# the answer.
	_flush_errors()
	var message := {"id": id, "ok": true}
	message.merge(fields)
	_send_json(message)

func _refuse(id, why: String) -> void:
	if id != null:
		_flush_errors()
		_send_json({"id": id, "error": why})

## A picture that answers request `id`: [u32 id][u16 width][u16 height][JPEG].
func _send_image(id, image: Image, quality: float) -> void:
	var jpeg := image.save_jpg_to_buffer(quality)
	var head := PackedByteArray()
	head.resize(8)
	head.encode_u32(0, int(id))
	head.encode_u16(4, image.get_width())
	head.encode_u16(6, image.get_height())
	_send(KIND_IMAGE, head + jpeg)

func _flush_errors() -> void:
	_errors_mutex.lock()
	var pending := _errors
	_errors = []
	_errors_mutex.unlock()
	for entry in pending:
		entry["event"] = "error"
		_send_json(entry)

# ---------- what Ghost asks ----------

func _handle(message: Dictionary) -> void:
	var id = message.get("id")
	# JSON gives every number as a float; an id goes back as the whole number it was.
	if id != null:
		id = int(id)
	match str(message.get("do", "")):
		"ack":
			_acked = max(_acked, int(message.get("frame", 0)))
		"watch":
			_watching = bool(message.get("on", true))
			_watch_max = clampi(int(message.get("max", 960)), 160, 1920)
			_watch_every = clampi(int(message.get("every", 2)), 1, 60)
			_acked = _sent
			_reply(id)
		"audio":
			AudioServer.set_bus_mute(0, not bool(message.get("on", false)))
			_reply(id)
		"time":
			_speed = clampf(float(message.get("speed", _speed)), 0.1, 4.0)
			if message.has("frozen"):
				_set_frozen(bool(message["frozen"]))
			_reply(id, {"frozen": _frozen, "speed": _speed})
		"input":
			for event in message.get("events", []):
				if event is Dictionary:
					_inject(event)
			_reply(id)
		"shot":
			_job_or_refuse(id, {"kind": "shot", "id": id, "max": int(message.get("max", 1568))})
		"act":
			_start_act(id, message)
		"wait":
			_start_wait(id, message)
		"perf":
			_job_or_refuse(id, {
				"kind": "perf", "id": id, "left": clampi(int(message.get("frames", 60)), 10, 600),
				"count": 0, "worst": 0, "total": 0, "last": 0, "was_frozen": _frozen,
			})
			if not _job.is_empty() and _job.get("id") == id:
				_set_frozen(false)
		"tree":
			_reply(id, _outline())
		"pick":
			_pick(id, message)
		"tune":
			_tune(id, message)
		"recent":
			_reply_recent(id, message)
		"replay":
			_start_replay(id, message)
		"inspect":
			_inspect(id, message)
		"set":
			_set_property(id, message)
		"eval":
			_eval(id, str(message.get("code", "")))
		"quit":
			_reply(id)
			get_tree().quit(0)
		_:
			_refuse(id, "This game's bridge does not know that request.")

func _job_or_refuse(id, job: Dictionary) -> void:
	if not _job.is_empty():
		_refuse(id, "The game is busy with another action. Wait for it to finish.")
		return
	_job = job

func _set_frozen(frozen: bool) -> void:
	if _frozen and not frozen:
		_deadline = 0
	_frozen = frozen

## Makes the tree what Ghost asked for. Called once a frame is drawn, so the game stops and
## starts between steps.
func _settle_frozen() -> void:
	if _frozen == _stopped:
		return
	if _frozen:
		_game_paused = get_tree().paused
		get_tree().paused = true
	else:
		get_tree().paused = _game_paused
	_stopped = _frozen

# ---------- input ----------

const _CODES := {
	"ArrowLeft": KEY_LEFT, "ArrowRight": KEY_RIGHT, "ArrowUp": KEY_UP, "ArrowDown": KEY_DOWN,
	"Space": KEY_SPACE, "Enter": KEY_ENTER, "NumpadEnter": KEY_KP_ENTER, "Escape": KEY_ESCAPE,
	"Tab": KEY_TAB, "Backspace": KEY_BACKSPACE, "Delete": KEY_DELETE, "Insert": KEY_INSERT,
	"Home": KEY_HOME, "End": KEY_END, "PageUp": KEY_PAGEUP, "PageDown": KEY_PAGEDOWN,
	"ShiftLeft": KEY_SHIFT, "ShiftRight": KEY_SHIFT, "ControlLeft": KEY_CTRL,
	"ControlRight": KEY_CTRL, "AltLeft": KEY_ALT, "AltRight": KEY_ALT, "MetaLeft": KEY_META,
	"MetaRight": KEY_META, "CapsLock": KEY_CAPSLOCK, "Minus": KEY_MINUS, "Equal": KEY_EQUAL,
	"BracketLeft": KEY_BRACKETLEFT, "BracketRight": KEY_BRACKETRIGHT, "Backslash": KEY_BACKSLASH,
	"Semicolon": KEY_SEMICOLON, "Quote": KEY_APOSTROPHE, "Backquote": KEY_QUOTELEFT,
	"Comma": KEY_COMMA, "Period": KEY_PERIOD, "Slash": KEY_SLASH,
}

## The Godot key for a name: a browser's `event.code` ("KeyA", "ArrowLeft"), or Godot's own name
## for a key ("A", "Left", "Space").
func _keycode(name: String) -> int:
	if _CODES.has(name):
		return _CODES[name]
	if name.length() == 4 and name.begins_with("Key"):
		return KEY_A + (name.unicode_at(3) - 65)
	if name.length() == 6 and name.begins_with("Digit"):
		return KEY_0 + (name.unicode_at(5) - 48)
	if name.begins_with("F") and name.substr(1).is_valid_int():
		return KEY_F1 + int(name.substr(1)) - 1
	if name.begins_with("Numpad") and name.substr(6).is_valid_int():
		return KEY_KP_0 + int(name.substr(6))
	return OS.find_keycode_from_string(name)

func _button(name: String) -> int:
	match name:
		"right":
			return MOUSE_BUTTON_RIGHT
		"middle":
			return MOUSE_BUTTON_MIDDLE
	return MOUSE_BUTTON_LEFT

## One input from the pane or from an action: a key, an input action, the mouse.
## Gives the game an input and writes it on the run's tape.
func _inject(event: Dictionary) -> bool:
	if not _apply(event):
		return false
	if _running:
		_record(event, _tick, 1)
	else:
		_record(event, _tick + 1, 0)
	return true

func _record(event: Dictionary, tick: int, phase: int) -> void:
	var code := str(event.get("code", "")).length() + str(event.get("value", "")).length()
	if _tape.size() >= MAX_TAPE or _tape_code + code > MAX_TAPE_CODE:
		_tape_full = true
		return
	_tape_code += code
	_tape.append([tick, phase, event])

## Writes on the tape something done now that is not input.
func _record_now(event: Dictionary) -> void:
	if _replaying:
		return
	if _running:
		_record(event, _tick, 1)
	else:
		_record(event, _tick + 1, 0)

func _apply(event: Dictionary) -> bool:
	var down := bool(event.get("down", true))
	match str(event.get("t", "")):
		# What a replay gives back besides input. Never taken from the pane: Ghost sends these
		# only as part of a tape this bridge wrote.
		"set":
			if not _replaying:
				return false
			_set_property(null, event)
		"eval":
			if not _replaying:
				return false
			_eval(null, str(event.get("code", "")))
		"pad":
			var pad := InputEventJoypadButton.new()
			pad.device = 0
			pad.button_index = clampi(int(event.get("button", 0)), 0, JOY_BUTTON_MAX - 1)
			pad.pressed = down
			pad.pressure = 1.0 if down else 0.0
			Input.parse_input_event(pad)
		"axis":
			var stick := InputEventJoypadMotion.new()
			stick.device = 0
			stick.axis = clampi(int(event.get("axis", 0)), 0, JOY_AXIS_MAX - 1)
			stick.axis_value = clampf(float(event.get("value", 0)), -1.0, 1.0)
			Input.parse_input_event(stick)
		"key":
			var keycode := _keycode(str(event.get("code", "")))
			if keycode == KEY_NONE:
				return false
			var key := InputEventKey.new()
			key.keycode = keycode
			key.physical_keycode = keycode
			key.pressed = down
			key.shift_pressed = bool(event.get("shift", false))
			key.ctrl_pressed = bool(event.get("ctrl", false))
			key.alt_pressed = bool(event.get("alt", false))
			var text := str(event.get("text", ""))
			if down and text.length() == 1:
				key.unicode = text.unicode_at(0)
			Input.parse_input_event(key)
		"action":
			var name := str(event.get("name", ""))
			if not InputMap.has_action(name):
				return false
			var action := InputEventAction.new()
			action.action = name
			action.pressed = down
			action.strength = 1.0 if down else 0.0
			Input.parse_input_event(action)
		"mouse":
			var click := InputEventMouseButton.new()
			click.position = Vector2(float(event.get("x", 0)), float(event.get("y", 0)))
			click.global_position = click.position
			click.button_index = _button(str(event.get("button", "left")))
			click.pressed = down
			click.double_click = bool(event.get("double", false))
			Input.parse_input_event(click)
		"move":
			var motion := InputEventMouseMotion.new()
			motion.position = Vector2(float(event.get("x", 0)), float(event.get("y", 0)))
			motion.global_position = motion.position
			motion.relative = Vector2(float(event.get("dx", 0)), float(event.get("dy", 0)))
			Input.parse_input_event(motion)
		"wheel":
			var up := float(event.get("dy", 0)) < 0
			for pressed in [true, false]:
				var wheel := InputEventMouseButton.new()
				wheel.position = Vector2(float(event.get("x", 0)), float(event.get("y", 0)))
				wheel.global_position = wheel.position
				wheel.button_index = MOUSE_BUTTON_WHEEL_UP if up else MOUSE_BUTTON_WHEEL_DOWN
				wheel.pressed = pressed
				Input.parse_input_event(wheel)
		_:
			return false
	return true

## The input that presses `name` down or lets it go: an input action of the project when it is
## one, else a key.
func _press(name: String, down: bool) -> bool:
	if InputMap.has_action(name):
		return _inject({"t": "action", "name": name, "down": down})
	return _inject({"t": "key", "code": name, "down": down})

# ---------- actions, waits and measurements ----------

func _frames_for(ms: float) -> int:
	return maxi(1, int(round(ms * 1000.0 / STEP_USEC)))

## An action: steps of input, each held for a stretch of game time. The game runs flat out while
## it lasts and is frozen again when it ends, and pictures are taken along the way.
func _start_act(id, message: Dictionary) -> void:
	var steps: Array = []
	var total := 0
	for raw in message.get("steps", []):
		if not raw is Dictionary:
			continue
		var step: Dictionary = raw
		var frames := _frames_for(float(step.get("ms", 100)))
		var names: Array = []
		if step.has("press"):
			names = [str(step["press"])]
		elif step.has("hold"):
			for name in step["hold"]:
				names.append(str(name))
		for name in names:
			if not InputMap.has_action(name) and _keycode(name) == KEY_NONE:
				_refuse(id, "\"%s\" is neither an input action of this project nor a key." % name)
				return
		steps.append({"names": names, "frames": frames, "click": step.get("click"),
			"button": str(step.get("button", "left")), "move": step.get("move")})
		total += frames
	if steps.is_empty():
		_refuse(id, "The action has no steps.")
		return
	if total > 60 * 120:
		_refuse(id, "The action is longer than two minutes of game time. Split it up.")
		return
	var shots := clampi(int(message.get("shots", 6)), 0, 12)
	_job_or_refuse(id, {
		"kind": "act", "id": id, "steps": steps, "index": -1, "left": 0, "done": 0,
		"total": total, "every": maxi(1, int(ceil(float(total) / maxf(1.0, shots)))),
		"shots": shots, "strip": [], "max": int(message.get("max", 1568)),
	})
	if not _job.is_empty() and _job.get("id") == id:
		_set_frozen(false)

func _start_wait(id, message: Dictionary) -> void:
	var expression := Expression.new()
	var code := str(message.get("until", ""))
	if expression.parse(code, PackedStringArray(["tree", "scene", "root"])) != OK:
		_refuse(id, "The condition could not be read: " + expression.get_error_text())
		return
	_job_or_refuse(id, {
		"kind": "wait", "id": id, "expression": expression,
		"left": _frames_for(clampf(float(message.get("timeout_ms", 5000)), 16, 120000)),
		"done": 0, "max": int(message.get("max", 1568)),
	})
	if not _job.is_empty() and _job.get("id") == id:
		_set_frozen(false)

func _release_held() -> void:
	for name in _held:
		_press(name, false)
	_held = []

## A replay: a run that has not taken a step yet is given a tape's inputs at the steps they were
## given, as fast as the game can go, and stands still at the end with pictures of its last
## stretch. The game is in the state the first run was in, as far as it draws no numbers and
## reads no clock of its own.
func _start_replay(id, message: Dictionary) -> void:
	if _tick != 0 or not _stopped:
		_refuse(id, "A replay starts with a run that has not taken a step yet.")
		return
	var events: Array = []
	for raw in message.get("events", []):
		if raw is Array and raw.size() == 3 and raw[2] is Dictionary:
			events.append([int(raw[0]), int(raw[1]), raw[2]])
	var until := clampi(int(message.get("until", 1)), 1, 60 * 60 * 30)
	var window := clampi(int(message.get("window", 900)), 1, until)
	var shots := clampi(int(message.get("shots", 12)), 0, 12)
	_job_or_refuse(id, {
		"kind": "replay", "id": id, "events": events, "next": 0, "until": until,
		"from": until - window, "shots": shots, "strip": [],
		"every": maxi(1, int(ceil(float(window) / maxf(1.0, shots)))),
		"max": int(message.get("max", 1568)),
	})
	if not _job.is_empty() and _job.get("id") == id:
		_set_frozen(false)

## Gives the game the tape's inputs that belong at `tick` and `phase`.
func _replay_events(tick: int, phase: int) -> void:
	var events: Array = _job["events"]
	var next := int(_job["next"])
	while next < events.size():
		var entry: Array = events[next]
		var at := int(entry[0])
		if at > tick or (at == tick and int(entry[1]) > phase):
			break
		if at == tick and int(entry[1]) == phase:
			_replaying = true
			var took := _apply(entry[2])
			_replaying = false
			if took:
				_record(entry[2], tick, phase)
		next += 1
	_job["next"] = next

## Lets go of everything a tape left pressed, so a game that is played on is not stuck with it.
func _release_tape() -> void:
	var down := {}
	for entry in _tape:
		var event: Dictionary = entry[2]
		var kind := str(event.get("t", ""))
		var key := ""
		if kind == "set" or kind == "eval":
			continue
		match kind:
			"key":
				key = "key:" + str(event.get("code", ""))
			"action":
				key = "action:" + str(event.get("name", ""))
			"mouse":
				key = "mouse:" + str(event.get("button", "left"))
			"pad":
				key = "pad:" + str(int(event.get("button", 0)))
			"axis":
				key = "axis:" + str(int(event.get("axis", 0)))
		if key == "":
			continue
		var held := bool(event.get("down", true))
		if kind == "axis":
			held = absf(float(event.get("value", 0))) > 0.001
		if held:
			down[key] = event
		else:
			down.erase(key)
	for key in down:
		var release: Dictionary = (down[key] as Dictionary).duplicate()
		release["down"] = false
		if release.get("t") == "axis":
			release["value"] = 0.0
		_inject(release)

## Once a frame, before the game's own nodes run.
func _advance_job() -> void:
	match str(_job.get("kind")):
		"act":
			if int(_job["left"]) == 0:
				var finished = _job.get("clicked")
				if finished != null:
					_inject({"t": "mouse", "x": finished[0], "y": finished[1],
						"button": _job.get("clicked_button", "left"), "down": false})
					_job.erase("clicked")
				_release_held()
				_job["index"] = int(_job["index"]) + 1
				var steps: Array = _job["steps"]
				if int(_job["index"]) >= steps.size():
					_job["finishing"] = true
					return
				var step: Dictionary = steps[int(_job["index"])]
				_job["left"] = int(step["frames"])
				var move = step.get("move")
				if move is Array and move.size() == 2:
					_inject({"t": "move", "x": move[0], "y": move[1]})
				var click = step.get("click")
				if click is Array and click.size() == 2:
					_inject({"t": "move", "x": click[0], "y": click[1]})
					_inject({"t": "mouse", "x": click[0], "y": click[1],
						"button": step["button"], "down": true})
					_job["clicked"] = click
					_job["clicked_button"] = step["button"]
				for name in step["names"]:
					_press(name, true)
					_held.append(name)
			_job["left"] = int(_job["left"]) - 1
			_job["done"] = int(_job["done"]) + 1
		"replay":
			_replay_events(_tick, 1)
			if _tick >= int(_job["until"]):
				_job["finishing"] = true
		"wait":
			var expression: Expression = _job["expression"]
			var scene := get_tree().current_scene
			var value = expression.execute([get_tree(), scene, get_tree().root], scene, false)
			if expression.has_execute_failed():
				_job["failed"] = expression.get_error_text()
				_job["finishing"] = true
			elif value:
				_job["met"] = true
				_job["finishing"] = true
			elif int(_job["left"]) <= 0:
				_job["finishing"] = true
			_job["left"] = int(_job["left"]) - 1
			_job["done"] = int(_job["done"]) + 1
		"perf":
			var now := Time.get_ticks_usec()
			if int(_job["last"]) != 0:
				var took := now - int(_job["last"])
				_job["total"] = int(_job["total"]) + took
				_job["worst"] = maxi(int(_job["worst"]), took)
				_job["count"] = int(_job["count"]) + 1
			_job["last"] = now
			_job["left"] = int(_job["left"]) - 1
			if int(_job["left"]) <= 0:
				_job["finishing"] = true

func _picture(limit: int) -> Image:
	var image := get_viewport().get_texture().get_image()
	var longest := maxi(image.get_width(), image.get_height())
	if longest > limit and limit > 0:
		var scale := float(limit) / float(longest)
		image.resize(maxi(1, int(image.get_width() * scale)), maxi(1, int(image.get_height() * scale)),
			Image.INTERPOLATE_BILINEAR)
	return image

## The pictures taken along an action, side by side in reading order, each small.
func _strip(frames: Array) -> Image:
	var cell: Image = frames[0]
	for frame in frames:
		# A window that changed size along the way: every cell takes the first one's.
		if (frame as Image).get_size() != cell.get_size():
			(frame as Image).resize(cell.get_width(), cell.get_height(), Image.INTERPOLATE_BILINEAR)
	var columns := mini(frames.size(), 4)
	var rows := int(ceil(float(frames.size()) / columns))
	var gap := 4
	var strip := Image.create_empty(columns * cell.get_width() + (columns - 1) * gap,
		rows * cell.get_height() + (rows - 1) * gap, false, Image.FORMAT_RGB8)
	strip.fill(Color(0.07, 0.07, 0.07))
	for index in frames.size():
		var frame: Image = frames[index]
		frame.convert(Image.FORMAT_RGB8)
		strip.blit_rect(frame, Rect2i(Vector2i.ZERO, frame.get_size()),
			Vector2i((index % columns) * (cell.get_width() + gap),
				int(index / columns) * (cell.get_height() + gap)))
	return strip

## After every frame is drawn: pictures for the pane and for the action under way, and then the
## game is stopped or started if Ghost asked for that during the frame.
func _after_draw() -> void:
	if _greeted:
		_drawn()
	_settle_frozen()

func _drawn() -> void:
	if not _job.is_empty():
		_finish_job_frame()
	if _running and _tick % RECENT_EVERY == 0:
		_recent.append([_tick, _picture(320)])
		if _recent.size() > RECENT_KEEP:
			_recent.pop_front()
	var due := _frame % (30 if _frozen and _job.is_empty() else _watch_every) == 0
	if _watching and due and _sent - _acked < 2:
		var image := _picture(_watch_max)
		var jpeg := image.save_jpg_to_buffer(0.75)
		_sent += 1
		var head := PackedByteArray()
		head.resize(9)
		head.encode_u32(0, _sent)
		head.encode_u16(4, image.get_width())
		head.encode_u16(6, image.get_height())
		head[8] = 1 if _frozen else 0
		_send(KIND_FRAME, head + jpeg)
	_since_stats += 1
	if _since_stats >= 30:
		_since_stats = 0
		var scene := get_tree().current_scene
		_send_json({
			"event": "stats", "frame": _frame, "frozen": _frozen, "speed": _speed,
			"fps": Performance.get_monitor(Performance.TIME_FPS),
			"nodes": int(Performance.get_monitor(Performance.OBJECT_NODE_COUNT)),
			"scene": scene.scene_file_path if scene != null else "",
			"paused_by_game": _game_paused if _stopped else get_tree().paused,
			"tick": _tick, "joypads": Input.get_connected_joypads().size(),
		})

func _finish_job_frame() -> void:
	var id = _job.get("id")
	match str(_job.get("kind")):
		"shot":
			var image := _picture(int(_job["max"]))
			_job = {}
			_reply(id, {"images": 1, "width": image.get_width(), "height": image.get_height(),
				"frozen": _frozen})
			_send_image(id, image, 0.9)
		"replay":
			if not _job.get("finishing", false):
				var seen: Array = _job["strip"]
				var since := _tick - int(_job["from"])
				if _running and int(_job["shots"]) > 0 and since > 0 \
						and since % int(_job["every"]) == 0 and seen.size() < 12:
					seen.append(_picture(360))
				# What was given while the game stood still before the next step goes in now,
				# after this frame and before that step's physics.
				_replay_events(_tick + 1, 0)
				return
			_set_frozen(true)
			var last := _picture(int(_job["max"]))
			var pictures: Array = _job["strip"]
			var given := int(_job["next"])
			_job = {}
			_reply(id, {"images": 2 if pictures.size() > 1 else 1, "frames": _tick,
				"game_ms": _tick * STEP_USEC / 1000.0, "inputs": given,
				"strip": pictures.size() if pictures.size() > 1 else 0,
				"width": last.get_width(), "height": last.get_height()})
			if pictures.size() > 1:
				_send_image(id, _strip(pictures), 0.8)
			_send_image(id, last, 0.9)
			_release_tape()
		"act":
			if not _job.get("finishing", false):
				# Not before the game has thawed: the first picture is of its first step.
				if not _running:
					return
				var done := int(_job["done"])
				var strip: Array = _job["strip"]
				if int(_job["shots"]) > 0 and done % int(_job["every"]) == 0 and strip.size() < 12:
					strip.append(_picture(360))
				return
			_set_frozen(true)
			var final := _picture(int(_job["max"]))
			var frames: Array = _job["strip"]
			var total := int(_job["total"])
			_job = {}
			_reply(id, {"images": 2 if frames.size() > 1 else 1, "frames": total,
				"game_ms": total * STEP_USEC / 1000.0, "strip": frames.size() if frames.size() > 1 else 0,
				"width": final.get_width(), "height": final.get_height()})
			if frames.size() > 1:
				_send_image(id, _strip(frames), 0.8)
			_send_image(id, final, 0.9)
		"wait":
			if not _job.get("finishing", false):
				return
			_set_frozen(true)
			var picture := _picture(int(_job["max"]))
			var done_frames := int(_job["done"])
			var failed = _job.get("failed")
			var met := bool(_job.get("met", false))
			_job = {}
			if failed != null:
				_refuse(id, "The condition failed while it ran: " + str(failed))
				return
			_reply(id, {"images": 1, "met": met, "frames": done_frames,
				"game_ms": done_frames * STEP_USEC / 1000.0,
				"width": picture.get_width(), "height": picture.get_height()})
			_send_image(id, picture, 0.9)
		"perf":
			if not _job.get("finishing", false):
				return
			var count := maxi(1, int(_job["count"]))
			var average := float(_job["total"]) / count / 1000.0
			var result := {
				"frames": count, "frame_ms": average, "worst_ms": int(_job["worst"]) / 1000.0,
				"could_run_at_fps": 1000.0 / maxf(average, 0.001),
				"physics_ms": Performance.get_monitor(Performance.TIME_PHYSICS_PROCESS) * 1000.0,
				"draw_calls": int(Performance.get_monitor(Performance.RENDER_TOTAL_DRAW_CALLS_IN_FRAME)),
				"objects_drawn": int(Performance.get_monitor(Performance.RENDER_TOTAL_OBJECTS_IN_FRAME)),
				"primitives": int(Performance.get_monitor(Performance.RENDER_TOTAL_PRIMITIVES_IN_FRAME)),
				"nodes": int(Performance.get_monitor(Performance.OBJECT_NODE_COUNT)),
				"orphan_nodes": int(Performance.get_monitor(Performance.OBJECT_ORPHAN_NODE_COUNT)),
				"memory_mb": Performance.get_monitor(Performance.MEMORY_STATIC) / 1048576.0,
				"video_memory_mb": Performance.get_monitor(Performance.RENDER_VIDEO_MEM_USED) / 1048576.0,
				"physics_2d_objects": int(Performance.get_monitor(Performance.PHYSICS_2D_ACTIVE_OBJECTS)),
				"physics_3d_objects": int(Performance.get_monitor(Performance.PHYSICS_3D_ACTIVE_OBJECTS)),
			}
			var was_frozen := bool(_job.get("was_frozen", false))
			_job = {}
			_set_frozen(was_frozen)
			_reply(id, result)

# ---------- the running scene ----------

func _where(node: Node) -> Dictionary:
	var out := {}
	if node is Control:
		var control := node as Control
		var rect := control.get_global_rect()
		var origin := control.get_global_transform_with_canvas().origin
		out["rect"] = [snappedf(origin.x, 0.1), snappedf(origin.y, 0.1),
			snappedf(rect.size.x, 0.1), snappedf(rect.size.y, 0.1)]
		out["visible"] = control.is_visible_in_tree()
	elif node is Node2D:
		var at := (node as Node2D).get_global_transform_with_canvas().origin
		out["at"] = [snappedf(at.x, 0.1), snappedf(at.y, 0.1)]
		out["visible"] = (node as Node2D).is_visible_in_tree()
	elif node is Node3D:
		var spatial := node as Node3D
		out["visible"] = spatial.is_visible_in_tree()
		var position := spatial.global_position
		out["world"] = [snappedf(position.x, 0.01), snappedf(position.y, 0.01), snappedf(position.z, 0.01)]
		var camera := get_viewport().get_camera_3d()
		if camera != null and not camera.is_position_behind(position):
			var at := camera.unproject_position(position)
			out["at"] = [snappedf(at.x, 0.1), snappedf(at.y, 0.1)]
	return out

## The last seconds of the run: its whole tape, and small pictures of the stretch asked for.
func _reply_recent(id, message: Dictionary) -> void:
	var seconds := clampf(float(message.get("seconds", 15)), 1.0, 15.0)
	var from := _tick - int(seconds * 60.0)
	var frames: Array = []
	for entry in _recent:
		if int(entry[0]) > from:
			frames.append((entry[1] as Image).duplicate())
	var chosen: Array = []
	if frames.size() > 12:
		for index in 12:
			chosen.append(frames[int(round(index * (frames.size() - 1) / 11.0))])
	else:
		chosen = frames
	# A run too young to have a picture in the ring still shows where it stands.
	if chosen.is_empty():
		chosen = [_picture(320)]
	_reply(id, {"images": 1, "tick": _tick, "events": _tape, "full": _tape_full,
		"pictures": chosen.size(), "seconds": minf(seconds, _tick / 60.0)})
	_send_image(id, _strip(chosen), 0.8)

func _box_of(points: Array) -> Array:
	var low := Vector2(INF, INF)
	var high := Vector2(-INF, -INF)
	for point in points:
		low = low.min(point)
		high = high.max(point)
	return [snappedf(low.x, 0.1), snappedf(low.y, 0.1),
		snappedf(high.x - low.x, 0.1), snappedf(high.y - low.y, 0.1)]

func _corners(transform: Transform2D, rect: Rect2) -> Array:
	return _box_of([transform * rect.position, transform * Vector2(rect.end.x, rect.position.y),
		transform * rect.end, transform * Vector2(rect.position.x, rect.end.y)])

## The box a node covers in the picture, [x, y, width, height], or nothing when it has no
## extent Ghost can tell (a plain Node2D, a node off screen, a node that is hidden).
func _box(node: Node) -> Array:
	if node is Control:
		var control := node as Control
		if not control.is_visible_in_tree() or control.size.x <= 0 or control.size.y <= 0:
			return []
		return _corners(control.get_global_transform_with_canvas(), Rect2(Vector2.ZERO, control.size))
	if node is CanvasItem:
		var item := node as CanvasItem
		if not item.is_visible_in_tree():
			return []
		var rect := Rect2()
		if node is Sprite2D:
			if (node as Sprite2D).texture != null:
				rect = (node as Sprite2D).get_rect()
		elif node is AnimatedSprite2D:
			var animated := node as AnimatedSprite2D
			var frames := animated.sprite_frames
			if frames != null and frames.has_animation(animated.animation) \
					and frames.get_frame_count(animated.animation) > 0:
				var texture := frames.get_frame_texture(animated.animation, animated.frame)
				if texture != null:
					var size := texture.get_size()
					rect = Rect2(animated.offset - (size / 2.0 if animated.centered else Vector2.ZERO), size)
		elif node is CollisionShape2D:
			var shape := (node as CollisionShape2D).shape
			if shape != null and shape.has_method("get_rect"):
				rect = shape.call("get_rect")
		elif node is Polygon2D:
			var polygon := (node as Polygon2D).polygon
			if polygon.size() >= 3:
				var low := Vector2(INF, INF)
				var high := Vector2(-INF, -INF)
				for corner in polygon:
					low = low.min(corner)
					high = high.max(corner)
				rect = Rect2(low + (node as Polygon2D).offset, high - low)
		elif node.is_class("TileMapLayer"):
			var used: Rect2i = node.call("get_used_rect")
			var tiles = node.get("tile_set")
			if tiles != null and used.size != Vector2i.ZERO:
				var tile := Vector2(tiles.get("tile_size"))
				rect = Rect2(Vector2(used.position) * tile, Vector2(used.size) * tile)
		if rect.size.x <= 0 or rect.size.y <= 0:
			return []
		return _corners(item.get_global_transform_with_canvas(), rect)
	if node is VisualInstance3D:
		var visual := node as VisualInstance3D
		var camera := get_viewport().get_camera_3d()
		if camera == null or not visual.is_visible_in_tree():
			return []
		var bounds := visual.get_aabb()
		if bounds.size == Vector3.ZERO:
			return []
		var points: Array = []
		for index in 8:
			var corner := visual.global_transform * bounds.get_endpoint(index)
			if camera.is_position_behind(corner):
				return []
			points.append(camera.unproject_position(corner))
		return _box_of(points)
	return []

func _find_at(node: Node, point: Vector2, state: Dictionary) -> void:
	if node == self or int(state["seen"]) >= 5000:
		return
	state["seen"] = int(state["seen"]) + 1
	var box := _box(node)
	if box.size() == 4 and point.x >= box[0] and point.y >= box[1] \
			and point.x <= box[0] + box[2] and point.y <= box[1] + box[3]:
		var area := float(box[2]) * float(box[3])
		# What fills most of the picture is the ground everything stands on: it is the answer
		# only when nothing smaller is under the pointer.
		var ground := area > float(state["screen"]) * 0.6
		var best = state["ground"] if ground else state["best"]
		if best == null or area <= float(best[1]):
			state["ground" if ground else "best"] = [node, area, box]
	for child in node.get_children():
		_find_at(child, point, state)

## The node under a point of the picture, for a person who points at what is wrong: what it is,
## where it is defined, and the script that drives it (its own, or the nearest above it).
func _pick(id, message: Dictionary) -> void:
	var point := Vector2(float(message.get("x", 0)), float(message.get("y", 0)))
	var size := get_viewport().get_visible_rect().size
	var state := {"seen": 0, "screen": size.x * size.y, "best": null, "ground": null}
	_find_at(get_tree().root, point, state)
	var found = state["best"] if state["best"] != null else state["ground"]
	if found == null:
		_reply(id, {"found": false})
		return
	var node: Node = found[0]
	var result := {"found": true, "path": str(node.get_path()), "name": str(node.name),
		"class": node.get_class(), "box": found[2]}
	var text := _text_of(node)
	if text != "":
		result["text"] = text
	var holder := node.owner if node.owner != null else node
	if holder.scene_file_path != "":
		result["scene"] = holder.scene_file_path
		result["in_scene"] = str(holder.get_path_to(node))
	var driven := node
	while driven != null:
		var script := driven.get_script() as Script
		if script != null and script.resource_path != "":
			result["script"] = script.resource_path
			result["script_on"] = str(driven.get_path())
			break
		driven = driven.get_parent()
	_reply(id, result)

## The values a person may turn while the game runs: what a node's script exports as a number
## or a switch, with the range its author gave.
func _tune(id, message: Dictionary) -> void:
	var node := _node_of(message)
	if node == null:
		_refuse(id, "No such node now.")
		return
	var script := node.get_script() as Script
	var values: Array = []
	for property in node.get_property_list():
		var usage := int(property["usage"])
		if not (usage & PROPERTY_USAGE_SCRIPT_VARIABLE and usage & PROPERTY_USAGE_EDITOR):
			continue
		var type := int(property["type"])
		if type != TYPE_FLOAT and type != TYPE_INT and type != TYPE_BOOL:
			continue
		var name := str(property["name"])
		var entry := {"name": name, "value": node.get(name),
			"type": "bool" if type == TYPE_BOOL else ("int" if type == TYPE_INT else "float")}
		if script != null:
			var first = script.get_property_default_value(name)
			if first != null:
				entry["default"] = first
		if int(property["hint"]) == PROPERTY_HINT_RANGE:
			var parts := str(property["hint_string"]).split(",")
			if parts.size() >= 2 and parts[0].is_valid_float() and parts[1].is_valid_float():
				entry["min"] = float(parts[0])
				entry["max"] = float(parts[1])
				if parts.size() >= 3 and parts[2].is_valid_float():
					entry["step"] = float(parts[2])
		values.append(entry)
		if values.size() >= 24:
			break
	_reply(id, {"path": str(node.get_path()), "name": str(node.name), "class": node.get_class(),
		"script": script.resource_path if script != null else "", "values": values})

func _text_of(node: Node) -> String:
	for property in ["text", "placeholder_text"]:
		if property in node:
			var value = node.get(property)
			if value is String and value != "":
				return value.substr(0, 80)
	return ""

## Every node of the running game with what it is and where it is on screen. Each gets a ref
## that the next request can name, until the next outline.
func _outline() -> Dictionary:
	_refs = {}
	var nodes: Array = []
	var state := {"count": 0, "cut": false}
	_walk(get_tree().root, 0, nodes, state)
	var scene := get_tree().current_scene
	return {"nodes": nodes, "cut": state["cut"],
		"scene": scene.scene_file_path if scene != null else ""}

func _walk(node: Node, depth: int, nodes: Array, state: Dictionary) -> void:
	if node == self:
		return
	if int(state["count"]) >= MAX_OUTLINE:
		state["cut"] = true
		return
	state["count"] = int(state["count"]) + 1
	var ref := "g%d" % state["count"]
	_refs[ref] = node.get_path()
	var entry := {"ref": ref, "depth": depth, "name": str(node.name), "class": node.get_class()}
	var script := node.get_script() as Script
	if script != null and script.resource_path != "":
		entry["script"] = script.resource_path
	if node.scene_file_path != "" and depth > 0:
		entry["scene"] = node.scene_file_path
	entry.merge(_where(node))
	var text := _text_of(node)
	if text != "":
		entry["text"] = text
	nodes.append(entry)
	var children := node.get_children()
	var shown := 0
	for child in children:
		if shown >= MAX_CHILDREN and children.size() > MAX_CHILDREN + 5:
			nodes.append({"depth": depth + 1, "more": children.size() - shown})
			break
		_walk(child, depth + 1, nodes, state)
		shown += 1

## The node a request names: a ref from the last outline, or a path from the root or the scene.
func _node_of(message: Dictionary) -> Node:
	var name := str(message.get("node", ""))
	if _refs.has(name):
		return get_node_or_null(_refs[name])
	if name.begins_with("/"):
		return get_node_or_null(NodePath(name))
	var scene := get_tree().current_scene
	if scene == null:
		return null
	if name == "" or name == ".":
		return scene
	return scene.get_node_or_null(NodePath(name))

func _shown(value) -> String:
	var text := var_to_str(value)
	return text if text.length() <= 240 else text.substr(0, 240) + "…"

func _inspect(id, message: Dictionary) -> void:
	var node := _node_of(message)
	if node == null:
		_refuse(id, "No such node now. Read the outline again: nodes come and go as the game runs.")
		return
	var properties := {}
	var script_properties := {}
	for property in node.get_property_list():
		var name := str(property["name"])
		var usage := int(property["usage"])
		if usage & PROPERTY_USAGE_SCRIPT_VARIABLE:
			if script_properties.size() < 80:
				script_properties[name] = _shown(node.get(name))
		elif usage & PROPERTY_USAGE_EDITOR and usage & PROPERTY_USAGE_STORAGE:
			var value = node.get(name)
			# What a person would look at first: plain values, not resources and sub-objects.
			if properties.size() < 60 and not value is Object and not value is Array and not value is Dictionary:
				properties[name] = _shown(value)
	var groups: Array = []
	for group in node.get_groups():
		if not str(group).begins_with("_"):
			groups.append(str(group))
	var script := node.get_script() as Script
	var result := {
		"path": str(node.get_path()), "class": node.get_class(),
		"script": script.resource_path if script != null else "",
		"groups": groups, "script_variables": script_properties, "properties": properties,
		"children": node.get_child_count(),
	}
	result.merge(_where(node))
	if node is CharacterBody2D:
		result["on_floor"] = (node as CharacterBody2D).is_on_floor()
		result["velocity"] = _shown((node as CharacterBody2D).velocity)
	elif node is CharacterBody3D:
		result["on_floor"] = (node as CharacterBody3D).is_on_floor()
		result["velocity"] = _shown((node as CharacterBody3D).velocity)
	_reply(id, result)

func _set_property(id, message: Dictionary) -> void:
	var node := _node_of(message)
	if node == null:
		_refuse(id, "No such node now. Read the outline again.")
		return
	var property := str(message.get("property", ""))
	if not property in node:
		_refuse(id, "%s has no property \"%s\"." % [node.get_class(), property])
		return
	var text := str(message.get("value", ""))
	var value = str_to_var(text)
	if value == null and text.strip_edges() != "null":
		value = text
	node.set(property, value)
	# By its path: the ref it may have been named by belongs to an outline that is gone later.
	_record_now({"t": "set", "node": str(node.get_path()), "property": property, "value": text})
	_reply(id, {"value": _shown(node.get(property))})

## Runs GDScript inside the game: one expression, or the body of a function that may `return`.
## `tree`, `scene` and `root` are there to use.
func _eval(id, code: String) -> void:
	_record_now({"t": "eval", "code": code})
	var scene := get_tree().current_scene
	var arguments := [get_tree(), scene, get_tree().root]
	if not code.contains("\n"):
		var expression := Expression.new()
		if expression.parse(code, PackedStringArray(["tree", "scene", "root"])) == OK:
			var value = expression.execute(arguments, scene, false)
			if not expression.has_execute_failed():
				_reply(id, {"value": _shown(value)})
				return
	var source := "extends RefCounted\nfunc run(tree: SceneTree, scene: Node, root: Window):\n"
	for line in code.split("\n"):
		source += "\t" + line + "\n"
	source += "\tpass\n"
	var script := GDScript.new()
	script.source_code = source
	if script.reload() != OK:
		_refuse(id, "The code could not be read as GDScript. The game's errors say where.")
		return
	var runner = script.new()
	var result = runner.call("run", arguments[0], arguments[1], arguments[2])
	_reply(id, {"value": _shown(result)})
