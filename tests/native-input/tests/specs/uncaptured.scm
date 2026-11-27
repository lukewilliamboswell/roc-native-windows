(test "carry uncaptured button state and drop movement across removal"
  (steps
    (pointer-down (image "Uncaptured") 500 1000)
    (pointer-move (image "Uncaptured") 1000 1000 9000 1000)
    (expect-absent (image "Uncaptured"))
    (expect-visible (text "first buttons: 1"))
    (expect-absent (text "stale move delivered"))))
