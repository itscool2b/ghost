extends Node2D

@export var speed := 200.0
var frames := 0

func _ready() -> void:
	print("fixture ready")

func _process(delta: float) -> void:
	frames += 1
	$Player.position.x += Input.get_axis("ui_left", "ui_right") * speed * delta
	if Input.is_action_just_pressed("ui_accept"):
		Globals.score += 1
	$Hud.text = "Score %d" % Globals.score

func explode() -> void:
	# Called by the test to make an error with a file and a line.
	var nothing: Node = null
	nothing.queue_free()
