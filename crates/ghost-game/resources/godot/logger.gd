# The game's errors, with their file and line, for Ghost (Godot 4.5 and later: `Logger`). Godot
# calls it from whichever thread the error came from, so it only hands the entry to the bridge,
# which sends it from the main thread.
extends Logger

var bridge: Object = null

func _log_error(function: String, file: String, line: int, code: String, rationale: String,
		_editor_notify: bool, error_type: int, script_backtraces: Array) -> void:
	if bridge == null:
		return
	var kind := "error"
	match error_type:
		ERROR_TYPE_WARNING:
			kind = "warning"
		ERROR_TYPE_SCRIPT:
			kind = "script"
		ERROR_TYPE_SHADER:
			kind = "shader"
	var entry := {
		"kind": kind,
		"text": rationale if rationale != "" else code,
		"detail": code if rationale != "" else "",
		"file": file,
		"line": line,
		"function": function,
	}
	# Where in the project's own scripts it happened, innermost first. Frames of Ghost's bridge
	# and of code Ghost was asked to run are not the project's, and are left out.
	var stack: Array = []
	var ran_by_ghost := false
	for backtrace in script_backtraces:
		if backtrace == null:
			continue
		for index in backtrace.get_frame_count():
			var frame_file: String = backtrace.get_frame_file(index)
			if not frame_file.begins_with("res://"):
				ran_by_ghost = true
				continue
			if stack.size() < 8:
				stack.append({
					"file": frame_file,
					"line": backtrace.get_frame_line(index),
					"function": backtrace.get_frame_function(index),
				})
	if not stack.is_empty():
		entry["stack"] = stack
	if not file.begins_with("res://"):
		# Raised in code Ghost ran (game_eval): point at the project's own frame when there is one.
		if stack.is_empty():
			entry["file"] = ""
			entry["line"] = 0
		else:
			entry["file"] = stack[0]["file"]
			entry["line"] = stack[0]["line"]
	if ran_by_ghost:
		entry["from_ghost"] = true
	bridge.call("report", entry)

func _log_message(_message: String, _error: bool) -> void:
	# Prints reach Ghost through the game's own output.
	pass
