extends Node2D

# DevicePower (addons/device_power, vendored from godot-device-power) sets the
# race's frame cap from the device's power state: the panel's full rate on a
# charger, 60 on battery or battery saver, lower when hot, and an Android game
# mode over the power source. Its Android plugin also asks the display for that
# rate, which lifts Android's 60 fps default for games. Menus still idle at
# frame::IDLE_FPS in Rust.

@onready var racer: Racer = $Racer


func _ready() -> void:
	DevicePower.changed.connect(_apply_power_cap)
	_apply_power_cap()


func _apply_power_cap() -> void:
	if not is_instance_valid(racer):
		return
	# Checked, not trusted: in a release export a script error anywhere in the
	# call returns null, and a null into the typed Rust method is a crash.
	var cap = DevicePower.recommended_fps_cap(DisplayServer.screen_get_refresh_rate())
	if cap is int:
		racer.set_platform_fps_cap(cap)
	print(
		"Device power: backend=%s source=%d saver=%s game_mode=%d thermal=%d back_off=%d rates=%s"
		% [
			DevicePower.get_backend_name(),
			DevicePower.get_power_source(),
			DevicePower.is_power_saver_enabled(),
			DevicePower.get_game_mode(),
			DevicePower.get_thermal_status(),
			DevicePower.get_thermal_back_off(),
			DevicePower.get_refresh_rates(),
		]
	)
