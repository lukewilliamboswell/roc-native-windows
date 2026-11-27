## Native libraries the app asks the host to register before mounting its UI.
Features := [].{
	Registration : () => U8
	Config : { register : List(Registration) }

	## Keep registration as data so composing app configuration is pure.
	use : List(Registration) -> Config
	use = |actions| { register: actions }

	## Enable Direct3D shader views. The host creates the device on first use.
	direct3d! : () => U8

	## Enable native HTTP transport. The app manifest still controls origins.
	http! : () => U8

	## Decode packaged and selected PNG, JPEG, and BMP images.
	image_codecs! : () => U8

	## Enable native file and folder dialogs, drops, and file operations.
	files! : () => U8

	## Run app-declared tasks on background Roc workers.
	workers! : () => U8

	## Publish progress on the app's Windows taskbar button.
	taskbar! : () => U8

	## Copy text from a native Copy button.
	clipboard! : () => U8

	## Enable editable data grids and their UI Automation providers.
	grid! : () => U8
}
