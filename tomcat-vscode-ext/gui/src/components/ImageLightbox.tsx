import { useEffect, useRef, useState } from "react";
import { useT } from "../i18n/LocaleProvider";
import { typedBlobUrl } from "../attachments/imagePipeline";

export interface ZoomedImage {
  alt: string;
  src: string;
  mimeType?: string;
}

function LightboxImage({ image }: { image: ZoomedImage }) {
  const t = useT();
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    if (image.mimeType !== "image/svg+xml") return;
    let cancelled = false;
    let ownedUrl: string | undefined;
    void typedBlobUrl(image.src, image.mimeType).then(value => {
      if (cancelled) { URL.revokeObjectURL(value); return; }
      ownedUrl = value; setUrl(value);
    }).catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; if (ownedUrl) URL.revokeObjectURL(ownedUrl); };
  }, [image.src, image.mimeType]);
  if (error) return <div role="alert">{t("image.preview.failed")}</div>;
  const src = image.mimeType === "image/svg+xml" ? url : image.src;
  if (!src) return <div role="status">{t("image.preview.loading")}</div>;
  return <img alt={image.alt} className="tc-image-lightbox__image" data-testid="image-lightbox-image"
    src={src} onMouseDown={event => event.stopPropagation()} onError={() => setError(true)} />;
}

export function ImageLightbox({
  image,
  onClose,
}: {
  image: ZoomedImage | null;
  onClose(): void;
}) {
  const t = useT();
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!image) {
      return;
    }
    previousFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeButtonRef.current?.focus();
    return () => {
      const previousFocus = previousFocusRef.current;
      if (previousFocus && previousFocus.isConnected) {
        previousFocus.focus();
      }
      previousFocusRef.current = null;
    };
  }, [image]);

  useEffect(() => {
    if (!image) {
      return;
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
    };
  }, [image, onClose]);

  if (!image) {
    return null;
  }

  return (
    <div
      className="tc-image-lightbox__overlay"
      data-testid="image-lightbox-overlay"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
      role="presentation"
    >
      <section
        aria-label={t("image.preview")}
        aria-modal="true"
        className="tc-image-lightbox"
        data-testid="image-lightbox"
        onMouseDown={(event) => {
          if (event.target === event.currentTarget) {
            onClose();
            return;
          }
          event.stopPropagation();
        }}
        role="dialog"
      >
        <button
          aria-label={t("image.preview.close")}
          className="tc-image-lightbox__close"
          data-testid="image-lightbox-close"
          onClick={onClose}
          ref={closeButtonRef}
          type="button"
        >
          <span aria-hidden="true" className="codicon codicon-close" />
        </button>
        <LightboxImage key={image.src} image={image} />
      </section>
    </div>
  );
}
