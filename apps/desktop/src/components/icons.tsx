import type { ReactNode, SVGProps } from "react";

/** Line icons drawn for Ghost at 16 px, stroked in the current text colour. Decorative only. */

type IconProps = Omit<SVGProps<SVGSVGElement>, "children">;

function Icon({ children, ...props }: IconProps & { readonly children: ReactNode }) {
  return (
    <svg
      width={16}
      height={16}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      {children}
    </svg>
  );
}

export const PlusIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M8 3.5v9M3.5 8h9" />
  </Icon>
);

export const FolderIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M2.5 4.5a1 1 0 0 1 1-1h2.6l1.4 1.5h4.9a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1h-8.9a1 1 0 0 1-1-1z" />
  </Icon>
);

export const SparkIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M8 2.5v2.2M8 11.3v2.2M2.5 8h2.2M11.3 8h2.2M4.1 4.1l1.5 1.5M10.4 10.4l1.5 1.5M4.1 11.9l1.5-1.5M10.4 5.6l1.5-1.5" />
  </Icon>
);

export const ChevronIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M6 4.5 9.5 8 6 11.5" />
  </Icon>
);

export const CheckIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="m3.5 8.3 2.8 2.7 6.2-6.3" />
  </Icon>
);

export const CrossIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="m4.5 4.5 7 7M11.5 4.5l-7 7" />
  </Icon>
);

export const FileIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M4 2.5h5l3 3v8a.5.5 0 0 1-.5.5h-7.5a.5.5 0 0 1-.5-.5v-10.5a.5.5 0 0 1 .5-.5z" />
    <path d="M9 2.5v3h3" />
  </Icon>
);

export const PencilIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="m10.3 3.2 2.5 2.5-7.2 7.2-3.1.6.6-3.1z" />
  </Icon>
);

export const TerminalIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="m3.5 5 3 3-3 3M8.5 11.5h4" />
  </Icon>
);

export const SearchIcon = (props: IconProps) => (
  <Icon {...props}>
    <circle cx="7" cy="7" r="3.8" />
    <path d="m9.8 9.8 3 3" />
  </Icon>
);

export const ToolIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M9.6 3.2a3 3 0 0 0 3.2 3.2l-6.4 6.4a1.4 1.4 0 0 1-2-2l6.4-6.4z" />
  </Icon>
);

export const CircleIcon = (props: IconProps) => (
  <Icon {...props}>
    <circle cx="8" cy="8" r="4.5" />
  </Icon>
);

export const SpinnerIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M8 3a5 5 0 1 0 5 5" />
  </Icon>
);

export const WarningIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M8 2.8 14 13H2z" />
    <path d="M8 6.5v3M8 11.2v.1" />
  </Icon>
);

export const ArrowLeftIcon = (props: IconProps) => (
  <Icon {...props}>
    <path d="M12.5 8h-9M7 4.5 3.5 8 7 11.5" />
  </Icon>
);
