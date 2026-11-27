import Clipboard
import Files
import System
import Ui

## Windows controls, effects, and packaged resources.
Win := [].{
	Clipboard : Clipboard
	Files : Files
	System : System
	Ui : Ui

	## Reference an asset declared in the app's assets.json catalog.
	asset : Str -> Str
	asset = |name| "asset-ref:${name}"
}
