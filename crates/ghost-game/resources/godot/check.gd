# Ghost's check of a Godot project without running it (game_check): started headless with
# `-s <this file>`, it loads every script and scene of the project, so each one that does not
# parse or load says so with its file and line in Godot's own words, then quits. Nothing is
# changed, and no window opens.
extends SceneTree

const MAX_FILES := 3000

func _initialize() -> void:
	var files: Array = []
	_collect("res://", files)
	var failed := 0
	for file in files:
		var resource = ResourceLoader.load(file, "", ResourceLoader.CACHE_MODE_IGNORE)
		if resource == null:
			failed += 1
			print("GHOST-CHECK-FAILED ", file)
		elif resource is Script and not (resource as Script).can_instantiate():
			# A script that loaded and still cannot be used did not compile.
			failed += 1
			print("GHOST-CHECK-FAILED ", file)
	print("GHOST-CHECK-DONE ", files.size(), " ", failed)
	quit(0)

func _collect(path: String, files: Array) -> void:
	var folder := DirAccess.open(path)
	if folder == null:
		return
	folder.list_dir_begin()
	var name := folder.get_next()
	while name != "":
		if files.size() >= MAX_FILES:
			break
		var full := path.path_join(name)
		if folder.current_is_dir():
			if not name.begins_with("."):
				_collect(full, files)
		elif name.ends_with(".gd") or name.ends_with(".tscn"):
			files.append(full)
		name = folder.get_next()
	folder.list_dir_end()
