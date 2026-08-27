import { useState, type ImgHTMLAttributes } from "react";

const DEFAULT_LOGO = "/logo.svg";

interface SiteLogoProps extends Omit<ImgHTMLAttributes<HTMLImageElement>, "src"> {
  src?: string;
}

export function SiteLogo({ src = "", ...props }: SiteLogoProps) {
  const requested = src.trim() || DEFAULT_LOGO;
  const [failedSource, setFailedSource] = useState("");
  const resolved = failedSource === requested ? DEFAULT_LOGO : requested;

  return <img
    {...props}
    src={resolved}
    onError={() => {
      // Do not retry the built-in fallback forever when it is unavailable.
      if (resolved !== DEFAULT_LOGO) setFailedSource(requested);
    }}
  />;
}
