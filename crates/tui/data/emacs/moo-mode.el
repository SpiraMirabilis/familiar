;;; moo-mode.el --- Major mode for MOO code (Stunt / LambdaMOO)  -*- lexical-binding: t; -*-

;; Built from Familiar's GtkSourceView spec (crates/gtk/data/language-specs/moo.lang):
;; LambdaMOO plus try/except, break/continue, maps, bitwise operators, /* comments */,
;; $#colour literals, b"bytes" literals and string escapes.
;;
;; Install: Familiar's `/editor syntax' writes this file to ~/.emacs.d/ (or ~/.config/emacs/).
;; Then add one line to your init file, with the path it was written to:
;;
;;   (load "~/.emacs.d/moo-mode.el")
;;
;; Files ending in .moo open in `moo-mode'.

;;; Code:

(defgroup moo nil
  "Editing MOO code."
  :group 'languages
  :prefix "moo-")

(defcustom moo-indent-offset 2
  "How many columns each block level is indented."
  :type 'integer
  :safe #'integerp)

;; Emacs 29 added faces for numbers, operators, escapes and function calls; older ones get the
;; nearest face they have.
(defun moo--face (new old)
  "NEW if this Emacs has it, otherwise OLD."
  (if (facep new) new old))

(defface moo-number-face `((t :inherit ,(moo--face 'font-lock-number-face 'font-lock-constant-face)))
  "Face for numbers in MOO code.")
(defface moo-operator-face `((t :inherit ,(moo--face 'font-lock-operator-face 'default)))
  "Face for operators in MOO code.")
(defface moo-escape-face `((t :inherit ,(moo--face 'font-lock-escape-face 'font-lock-regexp-grouping-backslash)))
  "Face for escapes like \\n in MOO strings.")
(defface moo-verb-call-face `((t :inherit ,(moo--face 'font-lock-function-call-face 'font-lock-function-name-face)))
  "Face for verb calls like x:tell( in MOO code.")
(defface moo-variable-face '((t :inherit font-lock-variable-name-face))
  "Face for MOO's built-in variables (player, this, args...).")

(defconst moo--builtins
  '(
    "abs" "acos" "add_property" "add_verb" "ancestors" "asin" "atan" "binary_hash"
    "binary_hmac" "binary_to_bytes" "boot_player" "buffered_output_length" "bytes_to_binary"
    "caller_perms" "callers" "ceil" "children" "chparent" "chparents" "chr" "clear_property"
    "color" "connected_players" "connected_seconds" "connection_name" "connection_option"
    "connection_options" "cos" "cosh" "crypt" "ctime" "db_disk_size" "decode"
    "decode_base64" "decode_binary" "decompiled_verb_code" "delete_property" "delete_verb"
    "descendants" "disassemble" "dump_database" "encode" "encode_base64" "encode_binary"
    "equal" "eval" "exec" "exp" "file_chmod" "file_close" "file_eof" "file_flush"
    "file_last_access" "file_last_change" "file_last_modify" "file_list" "file_mkdir"
    "file_mode" "file_name" "file_open" "file_openmode" "file_read" "file_readline"
    "file_readlines" "file_remove" "file_rename" "file_rmdir" "file_seek" "file_size"
    "file_stat" "file_tell" "file_type" "file_version" "file_write" "file_writeline"
    "floatstr" "floor" "flush_input" "force_input" "function_info" "gc_stats"
    "generate_json" "getenv" "idle_seconds" "index" "isa" "is_clear_property" "is_member"
    "is_player" "kill_task" "length" "listappend" "listdelete" "listen" "listeners"
    "listinsert" "listset" "load_server_options" "log" "log10" "log_cache_stats" "mapdelete"
    "mapkeys" "mapvalues" "match" "max" "max_object" "memory_usage" "min" "mysql_close"
    "mysql_connect" "mysql_connections" "mysql_escape_string" "mysql_field_names"
    "mysql_next_row" "mysql_ping" "mysql_query" "mysql_set_options" "mysql_status" "notify"
    "object_bytes" "open_network_connection" "ord" "output_delimiters" "parent" "parents"
    "parse_json" "pass" "players" "process_id" "properties" "property_info" "queued_tasks"
    "queue_info" "raise" "random" "random_bytes" "read" "read_http" "read_stdin" "renumber"
    "reset_max_object" "respond_to" "resume" "rindex" "rmatch" "run_gc" "salt"
    "seconds_left" "server_log" "server_version" "setadd" "set_connection_option"
    "set_player_flag" "set_property_info" "setremove" "set_task_local" "set_task_perms"
    "set_verb_args" "set_verb_code" "set_verb_info" "shutdown" "sin" "sinh" "sqlite_close"
    "sqlite_column_names" "sqlite_delete" "sqlite_finalize" "sqlite_handles"
    "sqlite_next_row" "sqlite_open" "sqlite_prepare" "sqlite_query" "sqlite_status" "sqrt"
    "strcmp" "string_hash" "string_hmac" "strsub" "strtr" "substitute" "suspend"
    "switch_player" "tan" "tanh" "task_id" "task_local" "task_perms" "task_stack"
    "ticks_left" "time" "tofloat" "toint" "toliteral" "tonum" "toobj" "tostr" "trunc"
    "typeof" "unlisten" "valid" "value_bytes" "value_hash" "value_hmac" "verb_args"
    "verb_cache_stats" "verb_code" "verb_info" "verbs")
  "Every built-in function in the Stunt server.")

(defconst moo--string-statement-re "^\\s-*\"\\(?:\\\\.\\|[^\"\\\\\n]\\)*\"\\s-*;\\s-*$"
  "A statement that is only a string: MOO's comment, by convention.")

(defconst moo--escape-re "\\\\\\(?:x[[:xdigit:]]\\{2\\}\\|u[[:xdigit:]]\\{4\\}\\|[nrte0\"\\\\]\\)")

(defun moo--escapes (limit)
  "Find the next string escape before LIMIT that's inside a string."
  (let (found)
    (while (and (not found) (re-search-forward moo--escape-re limit t))
      (setq found (save-match-data (save-excursion (nth 3 (syntax-ppss (match-beginning 0)))))))
    found))

(defun moo--bytes (limit)
  "Find the next b\" before LIMIT: only a lowercase b starts a bytes literal."
  (let ((case-fold-search nil))
    (re-search-forward "\\_<b\"" limit t)))

(defun moo--todos (limit)
  "Find the next TODO, FIXME or XXX before LIMIT in a comment."
  (let ((case-fold-search nil) found)
    (while (and (not found) (re-search-forward "\\_<\\(?:TODO\\|FIXME\\|XXX\\)\\_>" limit t))
      (setq found (save-match-data
                    (save-excursion
                      (or (nth 4 (syntax-ppss (match-beginning 0)))
                          (progn (beginning-of-line) (looking-at moo--string-statement-re)))))))
    found))

(defun moo--not-after-dot (words)
  "A regexp for WORDS, but not as property or verb names like x.player.
Group 1 is the word."
  (concat "\\(?:^\\|[^.:$]\\)\\(" (regexp-opt words 'symbols) "\\)"))

;; Earlier rules win where they overlap; those with t override strings and comments.
(defconst moo-font-lock-keywords
  `((moo--escapes 0 'moo-escape-face t)
    ;; MOO has no line comments; by convention a statement that is only a string is one.
    (,moo--string-statement-re 0 font-lock-comment-face t)
    (moo--todos 0 font-lock-warning-face t)
    (moo--bytes 0 font-lock-string-face keep)
    ("\\$#[[:alnum:]_]+" . font-lock-constant-face)
    ("#-?[0-9]+" . font-lock-constant-face)
    ("\\$[[:alpha:]_][[:alnum:]_]*" . font-lock-type-face)
    (":\\([[:alpha:]_][[:alnum:]_]*\\)\\s-*(" 1 'moo-verb-call-face)
    ;; Built-in functions only where they're called.
    (,(concat "\\(" (regexp-opt moo--builtins 'symbols) "\\)\\s-*(") 1 font-lock-builtin-face)
    (,(regexp-opt '("if" "else" "elseif" "endif" "for" "in" "endfor" "fork" "endfork" "return"
                    "while" "endwhile" "try" "except" "finally" "endtry" "break" "continue" "any")
                  'symbols)
     . font-lock-keyword-face)
    (,(regexp-opt '("E_NONE" "E_TYPE" "E_DIV" "E_PERM" "E_PROPNF" "E_VERBNF" "E_VARNF" "E_INVIND"
                    "E_RECMOVE" "E_MAXREC" "E_RANGE" "E_ARGS" "E_NACC" "E_INVARG" "E_QUOTA"
                    "E_FLOAT" "E_FILE" "E_EXEC" "E_INTRPT")
                  'symbols)
     . font-lock-constant-face)
    (,(moo--not-after-dot '("NUM" "OBJ" "STR" "LIST" "ERR" "INT" "FLOAT" "MAP" "ANON" "BYTES" "COLOR"))
     1 font-lock-type-face)
    (,(moo--not-after-dot '("player" "this" "caller" "verb" "args" "argstr" "dobj" "dobjstr"
                            "prepstr" "iobj" "iobjstr"))
     1 'moo-variable-face)
    ;; 1.5, .5 and 1e3; the 1 and 5 in 1..5 stay apart.
    ("\\(?:^\\|[^[:alnum:]_.]\\)\\(\\.[0-9]+\\(?:e[-+]?[0-9]+\\)?\\)" 1 'moo-number-face)
    ("\\_<[0-9]+\\(?:\\.[0-9]+\\)?\\(?:e[-+]?[0-9]+\\)?\\_>" . 'moo-number-face)
    ("->\\|=>\\|\\.\\.\\|&&\\|||\\|==\\|!=\\|<=\\|>=\\|<<\\|>>\\|[-+*/%^!~<>=?|@`']" . 'moo-operator-face))
  "What `moo-mode' highlights.")

(defvar moo-mode-syntax-table
  (let ((st (make-syntax-table)))
    (modify-syntax-entry ?_ "_" st)
    (modify-syntax-entry ?/ ". 14" st)
    (modify-syntax-entry ?* ". 23" st)
    (modify-syntax-entry ?\" "\"" st)
    (modify-syntax-entry ?\\ "\\" st)
    (dolist (c '(?' ?` ?$ ?# ?@ ?% ?& ?| ?^ ?! ?~ ?? ?+ ?- ?= ?< ?>))
      (modify-syntax-entry c "." st))
    st)
  "Syntax table for `moo-mode'.")

;;; Indentation: a block's lines go in a level, and its end (or else, except...) comes back out.

(defconst moo--opens-re
  "^\\s-*\\(?:if\\|elseif\\|else\\|for\\|while\\|fork\\|try\\|except\\|finally\\)\\_>")
(defconst moo--closes-re
  "^\\s-*\\(?:end\\(?:if\\|for\\|while\\|fork\\|try\\)\\|elseif\\|else\\|except\\|finally\\)\\_>")
(defconst moo--ends-re "\\_<end\\(?:if\\|for\\|while\\|fork\\|try\\)\\_>")

(defun moo--indent-after-previous ()
  "The indentation for a line after the nearest non-blank line above."
  (save-excursion
    (beginning-of-line)
    (let (found)
      (while (and (not found) (zerop (forward-line -1)))
        (setq found (not (looking-at "\\s-*$"))))
      (cond
       ((not found) 0)
       ;; A block that ends on the line it starts on (if (x) y; endif) doesn't indent.
       ((and (looking-at moo--opens-re)
             (not (re-search-forward moo--ends-re (line-end-position) t)))
        (+ (current-indentation) moo-indent-offset))
       (t (current-indentation))))))

(defun moo-indent-line ()
  "Indent the current line as MOO code."
  (interactive)
  (let* ((case-fold-search t)
         (closes (save-excursion (beginning-of-line) (looking-at moo--closes-re)))
         (target (max 0 (- (moo--indent-after-previous) (if closes moo-indent-offset 0)))))
    (if (<= (current-column) (current-indentation))
        (indent-line-to target)
      (save-excursion (indent-line-to target)))))

;;;###autoload
(define-derived-mode moo-mode prog-mode "MOO"
  "Major mode for MOO code, as the Stunt server accepts it."
  :syntax-table moo-mode-syntax-table
  ;; MOO keywords and names are case-insensitive.
  (setq-local font-lock-defaults '(moo-font-lock-keywords nil t))
  (setq-local comment-start "/* ")
  (setq-local comment-end " */")
  (setq-local comment-start-skip "/\\*+\\s-*")
  (setq-local indent-line-function #'moo-indent-line))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.moo\\'" . moo-mode))

(provide 'moo-mode)

;;; moo-mode.el ends here
