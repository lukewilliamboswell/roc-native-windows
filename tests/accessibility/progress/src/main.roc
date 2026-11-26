app [main, native] { pf: platform "../../../../platform/main.roc" }

import pf.Engine
import pf.Win

main = Engine.program(|| Engine.state({}, |_| Win.Ui.window({}, progress)))

native = Win.Features.use([])

progress = Win.Ui.progress_bar(Engine.const(Determinate({ completed: 1, total: 4 })), { label: "Transfer progress" })
