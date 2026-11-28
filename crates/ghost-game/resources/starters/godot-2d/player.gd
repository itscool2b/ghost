extends CharacterBody2D
## The player: the left and right arrows move, Enter or Space jumps.

const SPEED := 300.0
const JUMP := -520.0
const GRAVITY := 1400.0


func _physics_process(delta: float) -> void:
	if not is_on_floor():
		velocity.y += GRAVITY * delta
	var jump := Input.is_action_just_pressed("ui_accept") or Input.is_action_just_pressed("ui_select")
	if jump and is_on_floor():
		velocity.y = JUMP
	velocity.x = Input.get_axis("ui_left", "ui_right") * SPEED
	move_and_slide()
