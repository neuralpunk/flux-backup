<script lang="ts">
  // Explain before the system asks for an administrator password (docs/UX_PRINCIPLES.md, rule 5).
  import { app } from "../state.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";

  let { onclose }: { onclose: () => void } = $props();
</script>

<ConfirmDialog
  title="Manage system backups"
  icon="shield"
  confirm="Continue"
  onconfirm={() => {
    onclose();
    void app.unlockSystem();
  }}
  {onclose}
>
  <p>
    System backups protect what only the administrator can read: settings in <span class="font-mono text-[12.5px]">/etc</span>, other users' files, and
    the rest of the computer.
  </p>
  <p>
    To show and change them, Flux starts a small helper with administrator rights. <span class="font-medium text-fg">Your computer will now ask for an
    administrator password</span> (your own, if your account is an administrator).
  </p>
  <p class="text-fg-muted">The helper stops when you lock system backups, quit Flux, or after 15 minutes without use. Your own backups don't need it.</p>
</ConfirmDialog>
