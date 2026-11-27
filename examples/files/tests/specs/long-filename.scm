(test "a long filename keeps its complete caption"
  (setup (window-size 320 420))
  (steps
    (files "tests/fixtures/Café & Résumé — LongUnicodeFilenameWithNoSpacesToExerciseEllipsis.txt")
    (await-work)
    (expect-visible (text "Café & Résumé — LongUnicodeFilenameWithNoSpacesToExerciseEllipsis.txt"))
    (expect-visible (text "17 bytes"))
    (expect-text (text-area "Contents") "Unicode filename\n")))
