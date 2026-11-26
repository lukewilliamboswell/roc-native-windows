(test "show an error after the selected file cannot be opened"
  (steps
    (files "tests/fixtures/missing.txt")
    (expect-visible (text "Could not open the selected file."))
    (click (button "Clear"))
    (expect-visible (text "Choose a file to preview its contents."))))
