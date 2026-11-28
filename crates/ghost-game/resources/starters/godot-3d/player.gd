extends CharacterBody3D
## The player: the arrow keys move, Enter or Space jumps.

const SPEED := 5.0
const JUMP := 4.5
const GRAVITY := 9.8


func _physics_process(delta: float) -> void:
	if not is_on_floor():
		velocity.y -= GRAVITY * delta
	var jump := Input.is_action_just_pressed("ui_accept") or Input.is_action_just_pressed("ui_select")
	if jump and is_on_floor():
		velocity.y = JUMP
	var input := Input.get_vector("ui_left", "ui_right", "ui_up", "ui_down")
	velocity.x = input.x * SPEED
	velocity.z = input.y * SPEED
	move_and_slide()
