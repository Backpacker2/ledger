<script lang="ts">
	import { page } from '$app/state';
</script>

<div class="empty-state">
	<h1>{page.status === 404 ? 'Not found' : 'Something went wrong'}</h1>
	<p>{page.error?.message ?? 'Unknown error'}</p>
	<a class="btn primary" href="/">Back to the start</a>
</div>
