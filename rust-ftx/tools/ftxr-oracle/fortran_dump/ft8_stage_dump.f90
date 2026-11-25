program ft8_stage_dump
  include 'ft8_params.f90'

  integer, parameter :: NP2 = 2812
  integer, parameter :: MAXCAND = 1000

  character(len=512) :: in_path, out_dir, arg
  integer :: nfa_in, nfb_in, nfqso_in, max_samples
  integer :: nfa0, nfb0, ncand, ic, ncand_dump
  integer :: i, j, k, i0, ibest, idt, ifr, iloc(1), i1, is1, is2, is3, nsync
  integer :: ihalf, ks, nt, i3, ib, ip(1)
  integer :: ntype, nharderror, nbadcrc, did_sub
  integer*2 :: iwave(NMAX)
  real :: dd(NMAX), dd_sub_f(NMAX), dd_sub_t(NMAX)
  real :: sbase(NH1), candidate(3, MAXCAND)
  real :: sync21(21), ss(9), s8(0:7, NN), s2(0:7), bmeta(174), bmeta_raw(174), llra(174)
  real :: f1, xdt, smax, delf, delfbest, dphi, phi, twopi, dt2, fs2, sync, sync_fine, dmin
  real :: a(5), m1, m0, bm
  complex :: cd0(0:3199), cd0_ref(0:3199), ctwk(32), csymb(32), cs(0:7, NN)
  complex :: ca(NP2), cb(NP2)
  integer :: itone(NN)
  integer :: icos7(0:6), graymap(0:7)
  integer*1 :: apmask(174), cw(174), message91(91), message77(77)
  integer :: npatience, nthreads
  common/patience/npatience,nthreads

  data icos7/3,1,4,0,6,5,2/
  data graymap/0,1,3,2,5,6,4,7/

  npatience = 0
  nthreads = 1

  if (command_argument_count() .lt. 6) then
     write(0,*) 'usage: ft8_stage_dump <raw_i16.bin> <out_dir> <nfa> <nfb> <nfqso> <max_samples>'
     stop 1
  endif

  call get_command_argument(1, in_path)
  call get_command_argument(2, out_dir)
  call get_command_argument(3, arg); read(arg, *) nfa_in
  call get_command_argument(4, arg); read(arg, *) nfb_in
  call get_command_argument(5, arg); read(arg, *) nfqso_in
  call get_command_argument(6, arg); read(arg, *) max_samples

  if (max_samples .gt. NMAX) max_samples = NMAX
  if (max_samples .lt. 0) max_samples = 0

  iwave = 0
  open(unit=10, file=trim(in_path), access='stream', form='unformatted', status='old')
  read(10) iwave
  close(10)

  dd = 0.0
  do i = 1, max_samples
     dd(i) = real(iwave(i))
  enddo

  ! 1. Baseline
  nfa0 = nfa_in
  nfb0 = nfb_in
  call get_spectrum_baseline(dd, nfa0, nfb0, sbase)
  open(unit=11, file=trim(out_dir)//'/baseline.bin', access='stream', form='unformatted', status='replace')
  write(11) int(nfa0, 4), int(nfb0, 4), sbase
  close(11)

  ! 2. Sync8
  nfa0 = nfa_in
  nfb0 = nfb_in
  candidate = 0.0
  ncand = 0
  call sync8(dd, NMAX, nfa0, nfb0, 2.1, nfqso_in, MAXCAND, candidate, ncand, sbase)
  open(unit=12, file=trim(out_dir)//'/sync8.bin', access='stream', form='unformatted', status='replace')
  write(12) int(ncand, 4)
  if (ncand .gt. 0) then
     write(12) candidate(1:3, 1:ncand)
  endif
  close(12)

  ! 3-7. Top-5 candidate pipeline dumps + subtraction
  twopi = 8.0 * atan(1.0)
  fs2 = 12000.0 / NDOWN
  dt2 = 1.0 / fs2
  ncand_dump = min(ncand, 5)
  did_sub = 0

  do ic = 1, ncand_dump
     f1 = candidate(1, ic)
     xdt = candidate(2, ic)

     call ft8_downsample(dd, (ic .eq. 1), f1, cd0)

     i0 = nint((xdt + 0.5) * fs2)
     smax = 0.0
     ibest = 0
     ctwk = cmplx(1.0, 0.0)
     do idt = i0 - 10, i0 + 10
        call sync8d(cd0, idt, ctwk, 0, sync)
        sync21(idt - (i0 - 10) + 1) = sync
        if (sync .gt. smax) then
           smax = sync
           ibest = idt
        endif
     enddo

     smax = 0.0
     delfbest = 0.0
     do ifr = -5, 5
        delf = ifr * 0.5
        dphi = twopi * delf * dt2
        phi = 0.0
        do i = 1, 32
           ctwk(i) = cmplx(cos(phi), sin(phi))
           phi = mod(phi + dphi, twopi)
        enddo
        call sync8d(cd0, ibest, ctwk, 1, sync)
        if (sync .gt. smax) then
           smax = sync
           delfbest = delf
        endif
     enddo

     f1 = f1 + delfbest
     call ft8_downsample(dd, .false., f1, cd0_ref)

     do idt = -4, 4
        call sync8d(cd0_ref, ibest + idt, ctwk, 0, sync)
        ss(idt + 5) = sync
     enddo
     sync_fine = maxval(ss)
     iloc = maxloc(ss)
     ibest = iloc(1) - 5 + ibest
     xdt = (ibest - 1) * dt2

     do k = 1, NN
        i1 = ibest + (k - 1) * 32
        csymb = cmplx(0.0, 0.0)
        if (i1 .ge. 0 .and. i1 + 31 .le. NP2 - 1) csymb = cd0_ref(i1:i1+31)
        call four2a(csymb, 32, 1, -1, 1)
        cs(0:7, k) = csymb(1:8) / 1.0e3
        s8(0:7, k) = abs(csymb(1:8))
     enddo

     is1 = 0; is2 = 0; is3 = 0
     do k = 1, 7
        ip = maxloc(s8(:, k))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is1 = is1 + 1
        ip = maxloc(s8(:, k + 36))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is2 = is2 + 1
        ip = maxloc(s8(:, k + 72))
        if (icos7(k - 1) .eq. (ip(1) - 1)) is3 = is3 + 1
     enddo
     nsync = is1 + is2 + is3

     ! 1-symbol amplitude bmeta (nsym=1, imetric=1)
     bmeta_raw = 0.0
     do ihalf = 1, 2
        do k = 1, 29
           if (ihalf .eq. 1) ks = k + 7
           if (ihalf .eq. 2) ks = k + 43
           do i = 0, 7
              s2(i) = abs(cs(graymap(i), ks))
           enddo
           do ib = 0, 2
              m1 = -1.0e30
              m0 = -1.0e30
              do i = 0, 7
                 if (btest(i, 2 - ib)) then
                    if (s2(i) .gt. m1) m1 = s2(i)
                 else
                    if (s2(i) .gt. m0) m0 = s2(i)
                 endif
              enddo
              bm = m1 - m0
              bmeta_raw(1 + (k - 1) * 3 + (ihalf - 1) * 87 + ib) = bm
           enddo
        enddo
     enddo

     bmeta = bmeta_raw
     call normalizebmet(bmeta, 174)
     llra = 2.83 * bmeta

     apmask = 0
     cw = 0
     message91 = 0
     ntype = 0
     nharderror = -1
     dmin = 0.0
     call decode174_91(llra, 91, -1, 2, apmask, message91, cw, ntype, nharderror, dmin)

     write(arg, '(A,"/cand_",I1,".bin")') trim(out_dir), ic
     open(unit=13, file=trim(arg), access='stream', form='unformatted', status='replace')
     write(13) candidate(1, ic), candidate(2, ic), int(i0, 4), int(ibest, 4), delfbest, &
               f1, xdt, sync_fine, int(nsync, 4)
     write(13) cd0, cd0_ref, sync21, cs, s8, bmeta_raw, llra
     write(13) int(ntype, 4), int(nharderror, 4), dmin, cw, message91
     close(13)

     if (did_sub .eq. 0 .and. ntype .eq. 1 .and. nharderror .ge. 0 .and. count(cw .eq. 0) .ne. 174) then
        did_sub = 1
        message77 = message91(1:77)
        call get_ft8_tones_from_77bits(message77, itone)
        dd_sub_f = dd
        call subtractft8(dd_sub_f, itone, f1, xdt, .false.)
        dd_sub_t = dd
        call subtractft8(dd_sub_t, itone, f1, xdt, .true.)

        open(unit=14, file=trim(out_dir)//'/sub.bin', access='stream', form='unformatted', status='replace')
        write(14) int(ic, 4), f1, xdt, itone, dd_sub_f, dd_sub_t
        close(14)
     endif
  enddo

  ! 8. twkfreq1 test vector (npts=2812, fs=200.0)
  do i = 1, NP2
     ca(i) = cmplx(cos(0.1 * real(i)), sin(0.1 * real(i)))
  enddo
  a = (/ -1.5, 0.3, -0.05, 0.01, -0.002 /)
  call twkfreq1(ca, NP2, 200.0, a, cb)
  open(unit=15, file=trim(out_dir)//'/twkfreq.bin', access='stream', form='unformatted', status='replace')
  write(15) cb
  close(15)

end program ft8_stage_dump
