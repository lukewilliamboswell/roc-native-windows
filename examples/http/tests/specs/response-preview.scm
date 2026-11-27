(test "keep multiline and mixed-script response text at a narrow width"
  (setup (window-size 320 390))
  (steps
    (stub-http "plain" :url "https://example.test/plain" :status 200 :body "First response line with enough words to wrap.\n  Preserve the second line and its indentation.")
    (fill (text-field "URL") "https://example.test/plain")
    (press-enter (text-field "URL"))
    (run-effect 1)
    (expect-visible (text "First response line with enough words to wrap.\n  Preserve the second line and its indentation."))
    (stub-http "unicode" :url "https://example.test/unicode" :status 200 :body "Café\nこんにちは · שלום · ١٢٣")
    (fill (text-field "URL") "https://example.test/unicode")
    (press-enter (text-field "URL"))
    (run-effect 2)
    (expect-visible (text "Café\nこんにちは · שלום · ١٢٣"))))
