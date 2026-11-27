(demo "A reading list"
  (setup (window-size 480 470))
  (steps
    (screenshot "empty")
    (wait 400)
    (fill (text-field "Title") "Roc guide")
    (click (button "Add"))
    (wait 700)
    (click (button "Mark read"))
    (screenshot "finished")
    (wait 900)))
