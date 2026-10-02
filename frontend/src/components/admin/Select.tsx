import { Check, ChevronDown } from "lucide-react";
import { type CSSProperties, type KeyboardEvent, useId, useLayoutEffect, useRef, useState } from "react";

interface SelectOption {
  value: string;
  label: string;
  description?: string;
}

type SelectProps = {
  label: string;
  options: SelectOption[];
  disabled?: boolean;
} & ({ multiple?: false; value: string; onChange: (value: string) => void }
  | { multiple: true; value: string[]; placeholder: string; onChange: (value: string[]) => void });

/** Select-only control; the fixed menu stays aligned inside scrolling containers. */
export function Select({ label, options, disabled = false, ...props }: SelectProps) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [position, setPosition] = useState<CSSProperties>();
  const values = props.multiple ? props.value : [props.value];
  const selected = options.findIndex((option) => values.includes(option.value));
  const display = props.multiple
    ? options.filter((option) => values.includes(option.value)).map((option) => option.label).join(" / ") || props.placeholder
    : options[selected]?.label ?? props.value;

  useLayoutEffect(() => {
    if (!open) return;
    if (disabled) { setOpen(false); return; }
    function align() {
      const button = trigger.current;
      if (!button) return;
      const rect = button.getBoundingClientRect();
      const dialog = button.closest('[role="dialog"], .admin-content')?.getBoundingClientRect();
      if (dialog && (rect.bottom <= dialog.top || rect.top >= dialog.bottom)) {
        setOpen(false);
        return;
      }
      const below = window.innerHeight - rect.bottom - 12;
      const above = rect.top - 12;
      const upward = below < 280 && above > below;
      setPosition({
        left: rect.left,
        width: rect.width,
        maxHeight: Math.max(0, Math.min(280, upward ? above : below)),
        ...(upward ? { bottom: window.innerHeight - rect.top + 4 } : { top: rect.bottom + 4 }),
      });
    }
    function onScroll(event: Event) {
      if (event.target !== menu.current) align();
    }
    function outside(event: PointerEvent) {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    }
    align();
    window.addEventListener("resize", align);
    window.addEventListener("scroll", onScroll, true);
    document.addEventListener("pointerdown", outside);
    return () => {
      window.removeEventListener("resize", align);
      window.removeEventListener("scroll", onScroll, true);
      document.removeEventListener("pointerdown", outside);
    };
  }, [open, disabled]);

  useLayoutEffect(() => {
    const list = menu.current;
    const item = list?.children[active];
    if (!open || !list || !item) return;
    const bounds = list.getBoundingClientRect();
    const row = item.getBoundingClientRect();
    // Scroll only the options, not the containing editor or the page.
    if (row.top < bounds.top + 5) list.scrollTop -= bounds.top + 5 - row.top;
    else if (row.bottom > bounds.bottom - 5) list.scrollTop += row.bottom - bounds.bottom + 5;
  }, [open, active, position]);

  function show(index = Math.max(0, selected)) {
    setActive(index);
    setOpen(true);
  }

  function choose(index: number) {
    const option = options[index];
    if (!option) return;
    if (props.multiple) {
      props.onChange(values.includes(option.value) ? values.filter((value) => value !== option.value) : [...values, option.value]);
    } else {
      props.onChange(option.value);
      setOpen(false);
    }
    trigger.current?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (event.key === "Escape" && open) {
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
    } else if (event.key === "Tab") {
      setOpen(false);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (open) choose(active);
      else show();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) show();
      else setActive((index) => Math.max(0, Math.min(options.length - 1, index + (event.key === "ArrowDown" ? 1 : -1))));
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      show(event.key === "Home" ? 0 : options.length - 1);
    }
  }

  return (
    <div className="admin-select" ref={root} onBlur={(event) => {
      if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
    }}>
      <label htmlFor={id} id={`${id}-label`}><span>{label}</span></label>
      <button ref={trigger} id={id} type="button" className="admin-select-trigger" role="combobox" disabled={disabled}
        aria-labelledby={`${id}-label`} aria-expanded={open} aria-haspopup="listbox"
        aria-controls={open ? `${id}-menu` : undefined} aria-activedescendant={open ? `${id}-${active}` : undefined}
        onKeyDown={onKeyDown} onClick={() => open ? setOpen(false) : show()}>
        <span title={props.multiple ? display : props.value}>{display}</span><ChevronDown aria-hidden="true" />
      </button>
      {open ? <ul ref={menu} id={`${id}-menu`} className="admin-select-menu" role="listbox" aria-labelledby={`${id}-label`}
        aria-multiselectable={props.multiple || undefined}
        style={position ?? { visibility: "hidden" }} onPointerDown={(event) => event.preventDefault()}>
        {options.map((option, index) => <li key={option.value} id={`${id}-${index}`} role="option"
          aria-selected={values.includes(option.value)} data-active={index === active}
          onPointerMove={() => setActive(index)} onClick={() => choose(index)}>
          <span><span>{option.label}</span>{option.description ? <small>{option.description}</small> : !props.multiple && option.label !== option.value ? <small>{option.value}</small> : null}</span>
          {props.multiple ? <span className="admin-select-check" aria-hidden="true"><Check /></span> : <Check aria-hidden="true" />}
        </li>)}
      </ul> : null}
    </div>
  );
}
