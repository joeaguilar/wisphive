import { useEffect, useRef, type ReactNode } from "react";

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
}

export function Modal({ title, onClose, children }: ModalProps) {
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose]);

  return (
    <div className="modal-overlay" ref={overlayRef} onClick={(e) => {
      if (e.target === overlayRef.current) onClose();
    }}>
      <div className="modal-content">
        <div className="modal-header">
          <h2>{title}</h2>
          <button className="modal-close" onClick={onClose}>×</button>
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
