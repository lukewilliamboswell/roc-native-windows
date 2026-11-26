(demo "A reading list"
  (steps
    (screenshot "empty")
    (wait 400)
    (fill (text-field "Title") "Roc guide")
    (click (button "Add"))
    (wait 700)
    (toggle (checkbox "Roc guide"))
    (screenshot "finished")
    (wait 900)))
