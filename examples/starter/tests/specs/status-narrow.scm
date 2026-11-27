(test "keep the primary status pane at narrow window widths"
  (setup (window-size 160 480))
  (steps
    (expect-status "0 visible")
    (fill (text-field "Title") "A reading")
    (click (button "Add"))
    (expect-status "1 visible")))
