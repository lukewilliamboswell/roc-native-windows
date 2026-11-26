(test "add, read and remove a title"
  (steps
    (expect-visible (text "0 unread"))
    (fill (text-field "Title") "  Roc guide  ")
    (click (button "Add"))
    (expect-visible (checkbox "Roc guide"))
    (fill (text-field "Title") "Roc guide")
    (click (button "Add"))
    (expect-visible (text "1 unread"))
    (toggle (checkbox "Roc guide"))
    (expect-checked (checkbox "Roc guide") true)
    (expect-visible (text "0 unread"))
    (click (button "Remove"))
    (expect-absent (checkbox "Roc guide"))))
