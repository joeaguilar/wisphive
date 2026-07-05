import { useEffect, useId, useRef, type ReactNode } from "react";

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Extra class on the dialog surface (e.g. "help-modal"). */
  className?: string;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Modal({ title, onClose, children, className }: ModalProps) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    // Remember what had focus so we can restore it when the dialog closes.
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const content = contentRef.current;
    // Land focus on the dialog itself so screen readers announce it — unless a
    // child (e.g. a password field) already grabbed focus in its own effect.
    if (content && !content.contains(document.activeElement)) {
      content.focus();
    }

    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      // Trap Tab within the dialog.
      if (e.key !== "Tab" || !content) return;
      const focusables = [...content.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
        (el) => el.offsetParent !== null || el === document.activeElement,
      );
      if (focusables.length === 0) {
        e.preventDefault();
        content.focus();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement;
      if (e.shiftKey) {
        if (active === first || active === content) {
          e.preventDefault();
          last.focus();
        }
      } else if (active === last) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", handleKey, true);
    return () => {
      document.removeEventListener("keydown", handleKey, true);
      previouslyFocused?.focus?.();
    };
  }, [onClose]);

  return (
    <div className="modal-overlay" ref={overlayRef} onClick={(e) => {
      if (e.target === overlayRef.current) onClose();
    }}>
      <div
        className={`modal-content${className ? ` ${className}` : ""}`}
        ref={contentRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <div className="modal-header">
          <h2 id={titleId}>{title}</h2>
          <button className="modal-close" onClick={onClose} aria-label="Close dialog">×</button>
        </div>
        {children}
      </div>
    </div>
  );
}

interface TextModalProps {
  title: string;
  placeholder?: string;
  submitLabel: string;
  submitClass?: string;
  onSubmit: (text: string) => void;
  onClose: () => void;
}

export function TextModal({ title, placeholder, submitLabel, submitClass, onSubmit, onClose }: TextModalProps) {
  const textRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    textRef.current?.focus();
  }, []);

  const handleSubmit = () => {
    const text = textRef.current?.value.trim();
    if (text) onSubmit(text);
  };

  return (
    <Modal title={title} onClose={onClose}>
      <textarea
        ref={textRef}
        className="modal-textarea"
        aria-label={title}
        placeholder={placeholder}
        rows={4}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            handleSubmit();
          }
        }}
      />
      <div className="modal-actions">
        <button className={submitClass || "btn-approve"} onClick={handleSubmit}>
          {submitLabel}
        </button>
        <button className="btn-cancel" onClick={onClose}>Cancel</button>
      </div>
    </Modal>
  );
}

interface ConfirmModalProps {
  title: string;
  message: string;
  confirmLabel: string;
  confirmClass?: string;
  onConfirm: () => void;
  onClose: () => void;
}

export function ConfirmModal({ title, message, confirmLabel, confirmClass, onConfirm, onClose }: ConfirmModalProps) {
  return (
    <Modal title={title} onClose={onClose}>
      <p className="modal-message">{message}</p>
      <div className="modal-actions">
        <button className={confirmClass || "btn-approve"} onClick={onConfirm}>
          {confirmLabel}
        </button>
        <button className="btn-cancel" onClick={onClose}>Cancel</button>
      </div>
    </Modal>
  );
}
