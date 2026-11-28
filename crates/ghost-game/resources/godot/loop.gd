# Ghost's main loop for a Godot game it runs (docs/game.md). Godot is started with
# `-s <this file>`: it is a SceneTree like any game's, so the project's autoloads load as usual,
# and it adds Ghost's bridge before loading the project's own main scene. This file and the bridge
# live in Ghost's folder: nothing is written into the project.
extends SceneTree

func _initialize() -> void:
	var folder := (get_script() as Script).resource_path.get_base_dir()
	var bridge_script: Script = load(folder.path_join("bridge.gd"))
	if bridge_script == null:
		push_error("Ghost: the bridge script could not be loaded from " + folder)
		quit(70)
		return
	var bridge: Node = bridge_script.new()
	bridge.name = "GhostBridge"
	root.add_child(bridge)
	# A scene given to Ghost's Play (or to `game_run`) wins over the project's main scene.
	var scene := OS.get_environment("GHOST_GAME_SCENE")
	if scene == "":
		scene = ProjectSettings.get_setting("application/run/main_scene", "")
	if scene == "":
		push_error("Ghost: this project has no main scene. Set one in Project Settings > Application > Run, or name a scene to run.")
		quit(71)
		return
	var error := change_scene_to_file(scene)
	if error != OK:
		push_error("Ghost: the scene %s could not be loaded (%s)." % [scene, error_string(error)])
		quit(72)
